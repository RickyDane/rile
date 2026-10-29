//! The Rile Server service (`rile-server run`, started by systemd). Unix only.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crate::Paths;
use crate::daemon::{Daemon, Options};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

#[derive(Parser)]
#[command(name = "rile-server", version, about = "Shares this machine's folders with Rile apps on the local network")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server in the foreground (systemd runs this). SIGHUP reloads the configuration.
    Run {
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        #[arg(long, value_name = "DIR")]
        state: Option<PathBuf>,
        #[arg(long, value_name = "FILE")]
        socket: Option<PathBuf>,
        /// Listen on this port instead of the configured one (0 = any free port).
        #[arg(long)]
        port: Option<u16>,
        /// Don't announce on the network (devices must be added by address).
        #[arg(long)]
        no_mdns: bool,
    },
}

pub fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,mdns_sd=warn"))
        // journald adds its own timestamps.
        .format_timestamp(if std::env::var_os("INVOCATION_ID").is_some() { None } else { Some(env_logger::TimestampPrecision::Seconds) })
        .init();
    let Command::Run { config, state, socket, port, no_mdns } = Cli::parse().command;
    let paths = Paths::resolve(config, state, socket);
    let mut signals = Signals::new([SIGHUP, SIGINT, SIGTERM])?;
    let daemon = Daemon::start(Options { paths, port, mdns: !no_mdns }).inspect_err(|e| log::error!("{e:#}"))?;
    for signal in signals.forever() {
        match signal {
            SIGHUP => {
                if let Err(e) = daemon.reload() {
                    log::error!("reload failed, keeping the previous configuration: {e:#}");
                }
            }
            _ => break,
        }
    }
    log::info!("stopping");
    drop(daemon);
    Ok(())
}
