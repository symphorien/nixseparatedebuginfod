// SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
//
// SPDX-License-Identifier: GPL-3.0-only

#![warn(missing_docs)]

//! A server implementing the debuginfod protocol for nix packages.
//!
//! A [db::Cache] stores the buildid -> (source, debuginfo, executable) mapping.
//!
//! A [index::StoreWatcher] waits for new store paths to appears, and walks them
//! to populate the [db::Cache].
//!
//! Finally the [server] module provides server that serves the populated [db::Cache].

use std::{net::SocketAddr, process::ExitCode};

use clap::Parser;

use tikv_jemallocator::Jemalloc;

// makes RSS decrease after initial indexation, and decreases peak RSS during indexation
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

pub mod config;
pub mod db;
pub mod index;
pub mod log;
pub mod server;
pub mod store;
pub mod substituter;

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
async fn main() -> anyhow::Result<ExitCode> {
    match (
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("CACHE_DIRECTORY"),
    ) {
        (None, Some(dir)) => {
            // this env var is set by systemd
            std::env::set_var("XDG_CACHE_HOME", dir);
        }
        _ => (),
    }
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var(
            "RUST_LOG",
            "nixseparatedebuginfo=info,tower_http=debug,sqlx=warn,warn",
        )
    }
    let args = Options::parse();
    tracing_subscriber::fmt().without_time().init();

    // check that nix-store is present
    match store::detect_nix() {
        Err(e) => {
            tracing::error!("nix is not available: {:#}", e);
            return Ok(ExitCode::FAILURE);
        }
        Ok(()) => server::run_server(args).await,
    }
}
