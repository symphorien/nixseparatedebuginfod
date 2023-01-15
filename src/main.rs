use env_logger::Env;

mod db;
mod index;
mod log;
mod server;
mod store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init_from_env(Env::default().default_filter_or("warning"));

    server::run_server().await
}
