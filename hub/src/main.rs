mod api;
mod auth;
mod blob;
mod cli;
mod config;
mod db;
mod device;
mod health;
mod history;
mod relay;
mod server;
mod signaling;
mod tls;
mod web;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .compact()
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => cli::init(args),
        Command::Serve(args) => {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            rt.block_on(cli::serve(args))
        }
        Command::Migrate(args) => cli::migrate(args),
        Command::Backup(args) => cli::backup(args),
        Command::Doctor(args) => cli::doctor(args),
    }
}
