use crate::command::CommandProcessTracker;
use crate::router::Router;
use crate::server::http::handle_http_connections;
use crate::server::socks5::handle_tcp_connections;
use crate::stats::ConnectionMessage;
use log::{debug, error, info};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub mod http;
pub mod socks5;
mod transfer_data;
pub mod udp;
pub mod utils;

fn spawn_cleanup_tracker(tracker: Arc<CommandProcessTracker>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30)); // Run cleanup every 30 seconds
        loop {
            interval.tick().await;
            tracker.cleanup().await;
            debug!("Cleaned up terminated processes.");
        }
    })
}

pub async fn spawn_socks_server(
    socks_listen_addr: &str,
    http_listen_addr: &str,
    route_config: &PathBuf,
    mut shutdown_rx: watch::Receiver<bool>,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()> {
    let socks_listener = TcpListener::bind(socks_listen_addr).await?;
    let http_listener = TcpListener::bind(http_listen_addr).await?;
    info!(
        "Socks5 server listening on {}, HTTP proxy server listening on {}",
        socks_listen_addr, http_listen_addr
    );

    let router = Arc::new(Router::new(route_config).await?);

    let router_clone = Arc::clone(&router);

    let command_tracker = Arc::new(CommandProcessTracker::new());
    let cleanup_task = spawn_cleanup_tracker(command_tracker.clone());

    let config_rx = shutdown_rx.clone();
    let tcp_handler_rx = shutdown_rx.clone();

    let config_task = tokio::spawn(async move {
        if let Err(e) = router_clone.start_config_watcher(config_rx).await {
            error!("Failed to start config watcher: {:?}", e);
        }
    });

    let mut socks_task = tokio::spawn(handle_tcp_connections(
        socks_listener,
        router.clone(),
        command_tracker.clone(),
        tcp_handler_rx.clone(),
        stats_tx.clone(),
    ));

    let mut http_task = tokio::spawn(handle_http_connections(
        http_listener,
        router.clone(),
        shutdown_rx.clone(),
        command_tracker.clone(),
        stats_tx.clone(),
    ));

    tokio::select! {
        _ = shutdown_rx.changed() => {
            info!("Shutdown signal received. Stopping SOCKS5 and HTTP tasks...");
        }
        result = &mut socks_task => {
            if let Err(err) = result {
                error!("SOCKS5 task ended with an error: {:?}", err);
            }
        }
        result = &mut http_task => {
            if let Err(err) = result {
                error!("HTTP task ended with an error: {:?}", err);
            }
        }
    }

    if !socks_task.is_finished() {
        socks_task.abort(); // Abort the SOCKS5 task
    }
    if !http_task.is_finished() {
        http_task.abort(); // Abort the HTTP task
    }

    cleanup_task.abort();
    config_task.await?;

    Ok(())
}

pub(crate) async fn execute_command(
    tracker: Arc<CommandProcessTracker>,
    command: &str,
) -> anyhow::Result<()> {
    if tracker.is_running(command).await {
        debug!("Command `{}` is already running, skipping.", command);
        return Ok(());
    }

    let mut parts = command.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Invalid command"))?;
    let args: Vec<&str> = parts.collect();

    // Spawn the process
    let child = Command::new(program)
        .args(&args)
        .spawn()
        .map_err(|err| anyhow::anyhow!("Failed to start command `{}`: {:?}", command, err))?;

    debug!(
        "Command `{}` started with PID {}",
        command,
        child.id().unwrap_or(0)
    );

    tracker.add_process(command.to_string(), child).await;

    Ok(())
}
