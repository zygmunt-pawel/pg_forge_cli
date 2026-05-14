# appforge — design

**Date:** 2026-05-14
**Status:** design approved, pending spec review

## Summary

`appforge` is a single-host, hands-off deployment tool for stateless HTTP
web apps — the App Service / PaaS counterpart to pgforge (which is the
RDS counterpart). You build a Docker image locally, push the image
artifact to the host, and run one command that performs a zero-downtime
rollout with healthcheck-gated auto-rollback.

It is a sibling tool to pgforge, deliberately scoped the same way: single
host, no HA, no multi-host, no replication. appforge runs stateless apps;
pgforge runs the databases; other stateful dependencies run separately.

## Deployment model

- **Build happens on the developer laptop**, not on the host:
  `docker compose build` → `docker save` → transfer the image tar to the
  host (e.g. over ssh).
- **One command on the host** triggers the rollout: `appforge deploy`.
- No registry, no CI, no git remote on the host.

## App definition: compose file as input

The deployable unit is **one image + the run config for one web
service**. The user already writes a `docker-compose.yml` with exactly
one service (build, ports, environment, healthcheck, restart) — so the
compose file *is* the config. No separate `app.toml` format is invented.

- `appforge create --name X --compose docker-compose.yml` reads the
  single service and extracts only the fields appforge needs: image name,
  `ports` (internal port), `environment`, `restart`.
- The **HTTP healthcheck is not taken from compose** — compose's
  `healthcheck` is a shell command, not an HTTP path. The path and
  timeout that gate the rollout are passed explicitly to
  `appforge create` (`--health-path`, `--health-timeout`).
- These are written into appforge's own **normalized `state.toml`** per
  app. The compose file is an *input format only*; appforge does not
  support the full compose spec and does not re-read compose on deploy.
- **Exactly one service per compose file.** appforge zero-downtime-swaps
  exactly one web container. Databases live in pgforge; other stateful
  deps run separately and are reached via `environment`.

## Host architecture

`appforge` is a single native Rust binary (CLI + TUI), same ethos and
building blocks as pgforge. It runs on a single Linux host and manages
everything through Docker via bollard.

### System components (managed by appforge, default-on)

`appforge init` bootstraps the host: creates the `appforge` Docker
network and runs three **system containers** (official images, labelled
`appforge.system=true`):

- **Caddy** — local reverse proxy. The stable target that cloudflared
  points at; does hostname routing and the zero-downtime upstream swap.
  Supervised by appforge; its Caddyfile is generated from app state.
- **cloudflared** — Cloudflare Tunnel. Ingress for all apps across all
  domains. appforge manages its local `config.yml` ingress list and DNS
  routes.
- **Alloy** — Grafana Alloy telemetry collector. Gets the Docker socket
  + `/proc`/`/sys` for host and per-container metrics and logs. Ships
  with a default config; discovers app containers by label.

Opt-out per component via flags, e.g. `appforge init --no-alloy`.

### Request path

```
cloudflared (TLS terminated at Cloudflare edge)
  → Caddy (hostname routing + zero-downtime upstream swap)
    → app container (old or new during a deploy)
```

### Ingress: one tunnel, many domains

cloudflared is **not per-domain**. One tunnel serves many hostnames
across many domains, as long as every domain is a zone in the same
Cloudflare account. The tunnel's `ingress` list maps each hostname to
Caddy (one stable local address); Caddy does the per-app routing.

- `appforge create` adds an ingress rule (`hostname → Caddy`), creates
  the DNS CNAME (`cloudflared tunnel route dns`), reloads cloudflared.
- `appforge destroy` removes the rule + DNS record.

### cloudflared auth

appforge uses the **locally-managed tunnel** model (credentials file),
NOT the dashboard token model — the token model keeps ingress config in
the Cloudflare dashboard, which conflicts with appforge managing ingress
locally.

- **Prerequisite:** the user runs `cloudflared tunnel login` once on
  their laptop (browser auth), producing `cert.pem`.
- `appforge init --cert-pem ./cert.pem` transfers `cert.pem` to the
  host's state dir (tight perms, pgforge secrets pattern), then creates
  the tunnel, generates the credentials JSON and `config.yml`.
- `cert.pem` is portable / not machine-bound, so laptop-side login +
  ship-to-host is the clean flow and matches appforge's overall ethos.

## CLI surface

Mirrors pgforge's structure (clap derive, dispatch pattern). TUI when no
subcommand is given.

- `appforge init [--cert-pem PATH] [--no-alloy] ...` — bootstrap the host
- `appforge create --name X --compose docker-compose.yml --hostname H
  --health-path /health [--health-timeout SECS]` — register an app:
  normalize compose → state, add Caddy route, add cloudflared ingress +
  DNS
- `appforge deploy --name X --image X.tar` — the core rollout (below)
- `appforge rollback --name X` — redeploy the previous version
- `appforge ls` — all apps + status (running version, health)
- `appforge status --name X` — live metrics for one app
- `appforge logs --name X` — tail container logs
- `appforge destroy --name X` — remove app, container, Caddy route,
  cloudflared ingress + DNS

## State model

Per-app `state.toml` under the appforge state root (pgforge pattern:
atomic `update_under` / `load_under`):

- `current_deploy`, `previous_deploy` — deploy-ids
- `image_tag`, `internal_port`
- normalized run config (env, healthcheck path/timeout, restart, hostname)
- `last_deploy_at`, cached health status

The last N images per app are kept on disk for rollback; older images
are pruned after a successful deploy. `state.toml` is the source of
truth for reconciliation.

## Deploy flow

`appforge deploy --name api --image api.tar`:

1. **Load image** — `docker load` from the tar, tag
   `appforge/api:<deploy-id>` (deploy-id = timestamp + short hash). Load
   failure → nothing started, clean exit.
2. **Start new container** — `appforge_api_<deploy-id>` on the `appforge`
   network, fresh internal port, env + restart policy from normalized
   state, stamped with labels `appforge.app`, `appforge.deploy`,
   `appforge.metrics_port`.
3. **Healthcheck gate** — poll `GET http://<new>:<port><health_path>`
   until 200 or `health_timeout_s`, reusing pgforge's deadline-based
   wait loop (fail-fast if the container exits).
   - Timeout/failure → kill + remove the new container, leave old
     running, **deploy = FAIL**, exit non-zero. This *is* the
     auto-rollback — nothing was swapped.
4. **Swap traffic** — rewrite the Caddyfile (upstream for `api` → new
   container), `caddy validate` then `caddy reload` (graceful: drains
   in-flight connections, atomic upstream switch). cloudflared untouched.
   - `caddy validate` failure → abort, old keeps serving, kill new,
     report.
5. **Drain + retire old** — wait a configurable grace period (e.g. 10s)
   for old connections to finish, then stop + remove the old container.
   The old *image* is retained for rollback.
6. **Commit state** — update `state.toml` (`current_deploy` = new,
   `previous_deploy` = old, `last_deploy_at`), prune images beyond last N.

## Rollback

`appforge rollback --name api` runs steps 2–6 with the previous
deploy-id's image (still on disk, still recorded in `state.toml`).
Rollback is just "deploy the previous version" — no separate code path.

## Error handling & reconciliation

- **Healthcheck fail** → auto-rollback (new container killed, nothing
  swapped), deploy exits non-zero.
- **Caddy validate/reload fail** → abort, old keeps serving, new killed.
- **Image load fail** → nothing started, clean exit.
- **Old container won't stop** → log + force kill after grace period.
- **Crash mid-deploy** → `state.toml` is source of truth. Next `ls` /
  `deploy` reconciles: an `appforge.app` container whose deploy-id is
  neither `current` nor `previous` is an orphan and is removed; a missing
  `current` container shows unhealthy in `ls`. The Caddyfile is
  regenerated from state so it cannot drift.

## Observability

appforge does not run Alloy as an app concern — Alloy is a system
component (above). The integration is that **appforge makes itself
observable**: every container it creates is stamped with consistent
labels (`appforge.app`, `appforge.deploy`, `appforge.metrics_port`).

- Per-container CPU/memory/network metrics: isolation comes from
  containers (cgroups), not from the HTTP layer — each app is its own
  container and the kernel accounts it separately. Alloy's Docker
  discovery picks them up with clean `appforge_app` labels, queryable as
  separate series (`sum by (appforge_app) (...)`).
- Logs: Alloy's Docker log discovery, tagged by the same labels.
- App `/metrics`: scraped via the `appforge.metrics_port` label.
- Host metrics: pure Alloy, appforge uninvolved.
- During a deploy an app briefly has two containers (old + new, same
  `appforge.app`, different `appforge.deploy`) — summing by
  `appforge_app` double-counts for the overlap window; break down by
  `appforge_deploy` for precision.

## Reused from pgforge

- `docker/engine.rs` — engine trait (enables mocking Docker in tests)
- `docker/bollard_engine.rs` — bollard wrapper
- `docker/wait.rs` — deadline-based healthcheck/wait loop
- `state/instance.rs` pattern — per-app `state.toml`, atomic update/load
- `ports.rs` — internal port allocation
- CLI structure (clap derive) + `cli.rs` dispatch pattern
- TUI skeleton (ratatui) — list / detail / status views
- secrets handling — tight state-dir perms, redact-before-log
- `util/fs.rs`

## New code

- Caddy config generation (pure: app list → Caddyfile) + supervision
- cloudflared ingress generation + DNS routing + supervision
- Alloy default config + supervision
- `appforge init` host bootstrap (network + 3 system containers)
- deploy/swap state machine
- image transfer/load handling
- compose ingestion (one service → normalized state)

## Testing

- **Unit:** Caddy config generation, cloudflared ingress generation
  (pure functions), compose → normalized state parsing, `state.toml`
  round-trips.
- **Deploy state machine** against a mocked engine (via the engine
  trait): healthcheck-fail leaves old running, success swaps, crash
  reconciles.
- **Integration:** real deploy of a small test image with a `/health`
  endpoint; fire a continuous request stream during the swap and assert
  zero non-200 responses (proves zero-downtime).
- Heavy E2E does not auto-run in interactive sessions — compile + gated
  skip; run manually.

## Out of scope (YAGNI)

- TLS / certificates (cloudflared / Cloudflare edge handles it)
- Multi-service compose, workers, sidecars
- HA, multi-host, replication
- Building on the host
- Autoscaling
- Stateful dependencies (Postgres → pgforge; Redis etc. → run separately)
- Token-based (dashboard-managed) cloudflared tunnels — ingress would
  leave the host
- Alloy config-snippet generator — post-MVP convenience
