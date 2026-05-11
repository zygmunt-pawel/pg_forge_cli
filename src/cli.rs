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
        /// pgbackrest replication user password.
        #[arg(long, env = "PGFORGE_PGBACKREST_PASSWORD")]
        pgbackrest_password: String,
    },
}

fn parse_preset(s: &str) -> Result<Preset, String> {
    use std::str::FromStr;
    Preset::from_str(s)
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        None => {
            // TUI is added in Plan 5. Until then: print help.
            println!("pgforge: TUI not yet implemented (Plan 5). Run `pgforge --help`.");
            Ok(())
        }
        Some(Command::Create {
            name,
            preset,
            version,
            app_user,
            app_password,
            pgbackrest_password,
        }) => {
            let state = run_create(CreateArgs {
                name,
                preset,
                pg_version: version,
                app_user,
                app_password,
                pgbackrest_password,
                override_state_root: None,
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
