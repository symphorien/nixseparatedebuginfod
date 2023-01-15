use std::net::SocketAddr;

use clap::Parser;
use env_logger::Env;

mod db;
mod index;
mod log;
mod server;
mod store;

/// A debuginfod implementation that fetches debuginfo and sources from nix binary caches
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Options {
    /// Address for the server
    #[arg(short, long, default_value = "127.0.0.1:1949")]
    listen_address: SocketAddr,
    /// Only index the store and quit without serving
    #[arg(short, long)]
    index_only: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Options::parse();
    env_logger::init_from_env(Env::default().default_filter_or("warning"));

    server::run_server(args).await
}
