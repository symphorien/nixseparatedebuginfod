use std::net::SocketAddr;

use clap::Parser;

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
    match (std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("CACHE_DIRECTORY")) {
        (None, Some(dir)) => {
            // this env var is set by systemd
            std::env::set_var("XDG_CACHE_HOME", dir);
        },
        _ => ()
    }
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var(
            "RUST_LOG",
            "nixseparatedebuginfo=info,tower_http=debug,sqlx=warn,warn",
        )
    }
    let args = Options::parse();
    tracing_subscriber::fmt::init();

    server::run_server(args).await
}
