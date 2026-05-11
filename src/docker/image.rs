/// Render the Dockerfile that bakes pgbackrest + cron on top of the official
/// postgres image. Pure function — output is deterministic per pg_version.
pub fn dockerfile(pg_version: u8) -> String {
    format!(
        r#"FROM postgres:{ver}-bookworm

ENV DEBIAN_FRONTEND=noninteractive

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        pgbackrest cron tini ca-certificates tzdata; \
    rm -rf /var/lib/apt/lists/*; \
    mkdir -p /var/spool/pgbackrest /var/log/pgbackrest /etc/pgbackrest; \
    chown -R postgres:postgres /var/spool/pgbackrest /var/log/pgbackrest /etc/pgbackrest

# Tag of the image is set at build time by pgforge: pgforge/postgres:{ver}
"#,
        ver = pg_version
    )
}

/// Render the Dockerfile for an "upgrade" image that has BOTH `from_ver` and
/// `to_ver` postgres binaries installed plus pgbackrest. `pg_upgrade` (run
/// in a one-shot container built from this image) needs access to both
/// old-version and new-version binaries simultaneously.
///
/// Strategy: start from the `to_ver` image (so the active /usr/lib/postgresql/<to>
/// path points at the new version) and additionally `apt-get install
/// postgresql-<from>` from the same pgdg apt repo the official image
/// already configured.
pub fn upgrade_dockerfile(from_ver: u8, to_ver: u8) -> String {
    format!(
        r#"FROM postgres:{to}-bookworm

ENV DEBIAN_FRONTEND=noninteractive

# pgdg apt repo is already configured by the official postgres image, so we
# can install the OLD postgres binaries alongside the NEW image's binaries.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        postgresql-{from} \
        pgbackrest cron tini ca-certificates tzdata; \
    rm -rf /var/lib/apt/lists/*; \
    mkdir -p /var/spool/pgbackrest /var/log/pgbackrest /etc/pgbackrest; \
    chown -R postgres:postgres /var/spool/pgbackrest /var/log/pgbackrest /etc/pgbackrest

# Tag at build time: pgforge/upgrade:{from}-to-{to}
# Binaries: /usr/lib/postgresql/{from}/bin/  +  /usr/lib/postgresql/{to}/bin/
"#,
        from = from_ver,
        to = to_ver,
    )
}
