use crate::commands::create::{CreateArgs, run as run_create};
use crate::domain::preset::Preset;
use crate::error::Result;
use clap::{Parser, Subcommand};

#[derive(Subcommand, Debug)]
pub enum ScheduleAction {
    /// Install the launchd agent.
    Install,
    /// Remove the launchd agent.
    Uninstall,
    /// Show whether the agent plist exists + is currently loaded.
    Status,
}

#[derive(Parser, Debug)]
#[command(name = "pgforge", version, about = "Hardened single-host PostgreSQL provisioner")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Take a full backup of a running instance.
    Snapshot {
        /// Instance name. Either this OR --due must be given.
        #[arg(long, required_unless_present = "due")]
        name: Option<String>,
        /// Optional user-friendly label (stored alongside pgbackrest's label).
        #[arg(long)]
        label: Option<String>,
        /// Iterate every backup-enabled instance and snapshot the ones
        /// whose configured `snapshot_hour` has passed today and which
        /// haven't been snapshotted today yet. Intended for the LaunchAgent
        /// installed by `pgforge schedule install`.
        #[arg(long, conflicts_with_all = ["name", "label"])]
        due: bool,
    },
    /// List snapshots for an instance.
    Snapshots {
        #[arg(long)]
        name: String,
    },
    /// List all managed instances with status.
    Ls,
    /// Live metrics for one instance: CPU, memory, connections, sizes.
    Status {
        #[arg(long)]
        name: String,
    },
    /// Restore a backup of <source> as a NEW instance alongside it.
    Restore {
        /// Source instance name (whose backups to restore from).
        #[arg(long)]
        source: String,
        /// New instance name to create.
        #[arg(long = "as")]
        as_: String,
        /// Optional RFC3339 target time. Without it, the latest backup is used.
        #[arg(long)]
        target_time: Option<String>,
    },
    /// Clone a running instance as a NEW sibling via pg_basebackup.
    Clone {
        #[arg(long)]
        source: String,
        #[arg(long = "as")]
        as_: String,
    },
    /// Regenerate pg_hba.conf for an instance and reload PG (no restart).
    Reconfigure {
        #[arg(long)]
        name: String,
    },
    /// Recreate the container for an existing instance using current
    /// pgforge configs; keeps the data volume. Use after upgrading pgforge
    /// to apply new hardening to pre-existing instances.
    Rotate {
        #[arg(long)]
        name: String,
    },
    /// In-place major-version upgrade via pg_upgrade. Takes a pre-upgrade
    /// snapshot automatically (for backup-enabled instances) so the user
    /// can roll back via `pgforge restore`.
    Upgrade {
        #[arg(long)]
        name: String,
        #[arg(long = "to")]
        to_version: u8,
    },
    /// Manage the macOS LaunchAgent that runs `pgforge snapshot --due`
    /// every 5 minutes. Each instance opts in via its own
    /// `snapshot_hour` (set at Create time or with `pgforge cron`).
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
    /// Change an instance's auto-snapshot hour (LOCAL time, 0..=23) or
    /// disable it. `--hour 3` = daily at 03:00; `--off` = manual only.
    Cron {
        #[arg(long)]
        name: String,
        /// Hour of day to take the daily auto-snapshot. Mutually
        /// exclusive with --off.
        #[arg(long, conflicts_with = "off")]
        hour: Option<u8>,
        /// Disable auto-snapshot for this instance.
        #[arg(long, conflicts_with = "hour")]
        off: bool,
    },
    /// Change an instance's preset (tuning) — adjusts RAM limit,
    /// max_connections, shared_buffers etc. Rebuilds postgresql.conf
    /// and recreates the container with the new memory ceiling.
    /// Volume preserved (no data loss); ~10s downtime same as rotate.
    Resize {
        #[arg(long)]
        name: String,
        #[arg(long, value_parser = parse_preset)]
        preset: Preset,
    },
    /// Replace the running pgforge binary with the latest GitHub
    /// release. Atomic rename — safe even while the running TUI is
    /// holding the old binary in memory; the new version applies on
    /// next exec. Idempotent: prints "already on latest" when there's
    /// nothing newer.
    SelfUpdate {
        /// Re-download and replace even if local version matches the
        /// latest release tag.
        #[arg(long)]
        force: bool,
    },
    /// Permanently delete an instance: stop + remove container, drop
    /// data volume, remove state.toml. Optionally also wipe S3 backups
    /// (full + WAL archives + PITR window) via pgbackrest stanza-delete.
    Destroy {
        #[arg(long)]
        name: String,
        /// Also remove the pgbackrest stanza from S3 (full backups,
        /// diff backups, WAL archives — PITR becomes unrecoverable).
        /// Requires the container to be running so pgbackrest can be
        /// exec'd. Ignored for --no-backup instances.
        #[arg(long)]
        delete_backups: bool,
        /// Skip the interactive confirmation prompt. Required for
        /// scripts; for interactive use you should NOT pass this and
        /// type the instance name to confirm.
        #[arg(long)]
        yes: bool,
    },
    /// Create a new hardened PG instance.
    Create {
        /// Instance name (lowercase, [a-z][a-z0-9_-]{0,62}).
        #[arg(long)]
        name: String,
        /// Preset: tiny | small | medium | large.
        #[arg(long, value_parser = parse_preset)]
        preset: Preset,
        /// PostgreSQL major version (e.g. 18).
        #[arg(long)]
        version: u8,
        /// App user name. Default: leads.
        #[arg(long, default_value = "leads")]
        app_user: String,
        /// App password (set via env PGFORGE_APP_PASSWORD or this flag).
        #[arg(long, env = "PGFORGE_APP_PASSWORD")]
        app_password: String,
        /// pgbackrest replication user password. Not required with --no-backup.
        #[arg(long, env = "PGFORGE_PGBACKREST_PASSWORD", default_value = "")]
        pgbackrest_password: String,
        /// Skip pgbackrest setup — no S3, no WAL archiving, no snapshots,
        /// no clone/restore. Intended for local dev / tests where S3 is
        /// unavailable. The instance becomes a plain hardened PG with
        /// nothing to back it up.
        #[arg(long)]
        no_backup: bool,
        /// pgbackrest retention window in days. After each new full
        /// backup, pgbackrest deletes any full older than this along
        /// with the WAL needed to recover from it. 0 = keep forever.
        /// Default 30 ≈ RDS Standard retention.
        #[arg(long, default_value_t = 30)]
        retain_days: u32,
        /// Auto-snapshot hour (0..=23, local time). Requires `pgforge
        /// schedule install` to actually fire. Pass --no-snapshot-hour
        /// for manual-only.
        #[arg(long, default_value_t = 3)]
        snapshot_hour: u8,
        /// Disable auto-snapshot for this instance (manual only).
        #[arg(long, conflicts_with = "snapshot_hour")]
        no_snapshot_hour: bool,
    },
}

fn parse_preset(s: &str) -> Result<Preset, String> {
    use std::str::FromStr;
    Preset::from_str(s)
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        None => {
            use std::io::IsTerminal;
            if !std::io::stdout().is_terminal() {
                println!("pgforge: stdout is not a terminal. Run `pgforge --help` for commands.");
                return Ok(());
            }
            crate::tui::run().await
        }
        Some(Command::Snapshot { name, label, due }) => {
            if due {
                let n = crate::commands::snapshot::run_due(None).await?;
                println!("snapshot --due: {n} instance(s) snapshotted.");
                return Ok(());
            }
            let rec = crate::commands::snapshot::run(crate::commands::snapshot::SnapshotArgs {
                instance: name.expect("clap guard: --name required without --due"),
                user_label: label,
                override_state_root: None,
            })
            .await?;
            println!(
                "Snapshot taken: {} (label={:?}, taken_at={})",
                rec.label, rec.user_label, rec.taken_at
            );
            Ok(())
        }
        Some(Command::Snapshots { name }) => {
            let snaps = crate::commands::snapshots::run(&name, None)?;
            let had_snaps = !snaps.is_empty();
            if !had_snaps {
                println!("No snapshots for {name}.");
            } else {
                println!("{:<24}  {:<6}  {:<22}  {}", "label", "kind", "taken_at", "user_label");
                for s in &snaps {
                    println!(
                        "{:<24}  {:<6?}  {:<22}  {}",
                        s.label,
                        s.kind,
                        s.taken_at,
                        s.user_label.as_deref().unwrap_or("-")
                    );
                }
            }
            // PITR window — best-effort. Silently skip if the container is
            // down (snapshots list still printed) or pgbackrest info fails.
            let docker = crate::docker::bollard_engine::BollardEngine::connect()?;
            let state_root = crate::state::instance::InstanceState::default_state_root();
            match crate::commands::snapshots::pitr_window(&name, &docker, &state_root).await {
                Ok(w) if w.earliest.is_some() && w.latest.is_some() => {
                    println!(
                        "PITR window: {} .. {}",
                        w.earliest.unwrap(),
                        w.latest.unwrap()
                    );
                }
                Ok(_) if had_snaps => {
                    println!("PITR window: (unavailable — container not running, or pgbackrest info empty)");
                }
                Ok(_) => {} // no snaps + no window — we already said "No snapshots".
                Err(e) => eprintln!("(could not derive PITR window: {e})"),
            }
            Ok(())
        }
        Some(Command::Restore {
            source,
            as_,
            target_time,
        }) => {
            let state = crate::commands::restore::run(crate::commands::restore::RestoreArgs {
                source,
                as_name: as_,
                target_time,
                override_state_root: None,
            })
            .await?;
            let i = &state.instance;
            println!(
                "Restored instance ready:\n  postgresql://{}:***@127.0.0.1:{}/{}",
                i.app_user, i.host_port, i.db_name
            );
            Ok(())
        }
        Some(Command::Clone { source, as_ }) => {
            let state = crate::commands::clone::run(crate::commands::clone::CloneArgs {
                source,
                as_name: as_,
                override_state_root: None,
            })
            .await?;
            let i = &state.instance;
            println!(
                "Clone ready:\n  postgresql://{}:***@127.0.0.1:{}/{}",
                i.app_user, i.host_port, i.db_name
            );
            Ok(())
        }
        Some(Command::Reconfigure { name }) => {
            crate::commands::reconfigure::run(crate::commands::reconfigure::ReconfigureArgs {
                instance: name.clone(),
                override_state_root: None,
            })
            .await?;
            println!("Reconfigured {name}.");
            Ok(())
        }
        Some(Command::Ls) => {
            let rows = crate::commands::ls::run(crate::commands::ls::LsArgs {
                override_state_root: None,
            })
            .await?;
            print!("{}", crate::commands::ls::render_table(&rows));
            Ok(())
        }
        Some(Command::Status { name }) => {
            let s = crate::commands::status::run(crate::commands::status::StatusArgs {
                name,
                override_state_root: None,
            })
            .await?;
            print!("{}", crate::commands::status::render(&s));
            Ok(())
        }
        Some(Command::Rotate { name }) => {
            crate::commands::rotate::run(crate::commands::rotate::RotateArgs {
                name: name.clone(),
                override_state_root: None,
            })
            .await?;
            println!("Rotated {name}. Container recreated with current configs; volume retained.");
            Ok(())
        }
        Some(Command::Upgrade { name, to_version }) => {
            crate::commands::upgrade::run(crate::commands::upgrade::UpgradeArgs {
                name: name.clone(),
                to_version,
                override_state_root: None,
            })
            .await?;
            println!("Upgraded {name} to PostgreSQL {to_version}.");
            Ok(())
        }
        Some(Command::Resize { name, preset }) => {
            let (old, new) = crate::commands::resize::run(crate::commands::resize::ResizeArgs {
                name: name.clone(),
                new_preset: preset,
                override_state_root: None,
            })
            .await?;
            println!("Resized {name}: {old:?} → {new:?} (container recreated, volume retained).");
            Ok(())
        }
        Some(Command::SelfUpdate { force }) => {
            let out = crate::commands::self_update::run(force).await?;
            if out.upgraded {
                println!("Upgraded {} → {}. Restart pgforge to use the new binary.",
                    out.current_version, out.latest_tag);
            } else {
                println!("Already on {}.", out.current_version);
            }
            Ok(())
        }
        Some(Command::Destroy { name, delete_backups, yes }) => {
            // Interactive guard: load state to confirm instance exists and
            // print what's about to be destroyed, then require the user to
            // type the instance name back. Skipped with --yes.
            let state_root = crate::state::instance::InstanceState::default_state_root();
            let state = crate::state::instance::InstanceState::load_under(&state_root, &name)?;
            if !yes {
                use std::io::{BufRead, Write};
                eprintln!("Destroy instance '{name}'. This will delete:");
                eprintln!("  • docker container pgforge_{name}");
                eprintln!("  • docker volume {} (LOCAL DATA LOSS)", state.instance.volume_name());
                eprintln!("  • state dir + secrets under {}/instances/{name}/", state_root.display());
                if delete_backups {
                    if state.instance.backup_enabled {
                        eprintln!("  • ALL S3 backups for this instance (full + diff + WAL archives, NO PITR recoverable afterwards)");
                    } else {
                        eprintln!("  (--delete-backups is a no-op: instance is --no-backup)");
                    }
                } else if state.instance.backup_enabled {
                    eprintln!("  (S3 backups are RETAINED — re-use with `pgforge restore` later, or destroy them with `--delete-backups`)");
                }
                eprint!("Type the instance name '{name}' to confirm (or anything else to abort): ");
                std::io::stderr().flush().ok();
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line).map_err(|e| {
                    crate::error::PgForgeError::Anyhow(anyhow::anyhow!("stdin read: {e}"))
                })?;
                if line.trim() != name {
                    eprintln!("Aborted — name did not match.");
                    return Ok(());
                }
            }
            crate::commands::destroy::run(crate::commands::destroy::DestroyArgs {
                name: name.clone(),
                delete_backups,
                override_state_root: None,
            })
            .await?;
            println!("Destroyed {name}.");
            Ok(())
        }
        Some(Command::Schedule { action }) => {
            match action {
                ScheduleAction::Install => {
                    let path = crate::commands::schedule::install()?;
                    println!("Installed LaunchAgent at {}.", path.display());
                    println!("Each instance fires according to its `snapshot_hour` field — `pgforge cron --name X --hour N` to change.");
                    Ok(())
                }
                ScheduleAction::Uninstall => {
                    crate::commands::schedule::uninstall()?;
                    println!("Uninstalled.");
                    Ok(())
                }
                ScheduleAction::Status => {
                    let s = crate::commands::schedule::status()?;
                    println!("plist:  {}", s.plist_path.display());
                    println!("on disk: {}", s.plist_present);
                    println!("loaded:  {}", s.loaded);
                    Ok(())
                }
            }
        }
        Some(Command::Cron { name, hour, off }) => {
            if let Some(h) = hour {
                crate::domain::instance::Instance::validate_snapshot_hour(h)?;
            }
            let root = crate::state::instance::InstanceState::default_state_root();
            let mut state = crate::state::instance::InstanceState::load_under(&root, &name)?;
            state.instance.snapshot_hour = if off { None } else { hour };
            state.save_under(&root)?;
            println!("Updated {name}: snapshot_hour = {:?}", state.instance.snapshot_hour);
            Ok(())
        }
        Some(Command::Create {
            name,
            preset,
            version,
            app_user,
            app_password,
            pgbackrest_password,
            no_backup,
            retain_days,
            snapshot_hour,
            no_snapshot_hour,
        }) => {
            if !no_snapshot_hour {
                crate::domain::instance::Instance::validate_snapshot_hour(snapshot_hour)?;
            }
            let state = run_create(CreateArgs {
                name,
                preset,
                pg_version: version,
                app_user,
                app_password,
                pgbackrest_password,
                override_state_root: None,
                no_backup,
                retain_days,
                snapshot_hour: if no_snapshot_hour { None } else { Some(snapshot_hour) },
            })
            .await?;
            let i = &state.instance;
            println!(
                "Instance ready:\n  postgresql://{}:***@127.0.0.1:{}/{}",
                i.app_user, i.host_port, i.db_name
            );
            Ok(())
        }
    }
}
