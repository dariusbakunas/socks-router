use anyhow::Result;
use clap::Parser;
use socks_router::cli::Cli;
use socks_router::socks5::server::spawn_socks_server;
use tokio::signal;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let cli = Cli::parse();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let shutdown_task = tokio::spawn(handle_shutdown_signal(shutdown_tx.clone()));

    spawn_socks_server(&cli.listen_addr, &cli.route_config, shutdown_rx).await?;

    shutdown_task.await?;

    Ok(())
}

pub async fn handle_shutdown_signal(shutdown_tx: watch::Sender<bool>) {
    // Wait for a shutdown signal (e.g., Ctrl+C)
    signal::ctrl_c()
        .await
        .expect("Failed to listen for shutdown signal");

    // Send the shutdown notification to all receivers
    let _ = shutdown_tx.send(true);
}
