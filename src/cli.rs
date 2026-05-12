use crate::commands::create::{CreateArgs, run as run_create};
use crate::domain::preset::Preset;
use crate::error::Result;
use clap::{Parser, Subcommand};

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
        /// Instance name.
        #[arg(long)]
        name: String,
        /// Optional user-friendly label (stored alongside pgbackrest's label).
        #[arg(long)]
        label: Option<String>,
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
        Some(Command::Snapshot { name, label }) => {
            let rec = crate::commands::snapshot::run(crate::commands::snapshot::SnapshotArgs {
                instance: name,
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
        Some(Command::Create {
            name,
            preset,
            version,
            app_user,
            app_password,
            pgbackrest_password,
            no_backup,
        }) => {
            let state = run_create(CreateArgs {
                name,
                preset,
                pg_version: version,
                app_user,
                app_password,
                pgbackrest_password,
                override_state_root: None,
                no_backup,
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
