use anyhow::Result;
use clap::Parser;
use log::{debug, error, info};
use socks_router::cli::Cli;
use socks_router::command::CommandProcessTracker;
use socks_router::router::route_cache::RouteCache;
use socks_router::router::route_config::read_routing_config;
use socks_router::router::router::Router;
use socks_router::socks5::server::serve_socks5;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{watch, RwLock};
use tokio::task;
use tokio::task::JoinHandle;
use tokio::{signal, time};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    spawn_socks_server().await
}

async fn spawn_socks_server() -> Result<()> {
    let cli = Cli::parse();
    let listener = TcpListener::bind(&cli.listen_addr).await?;
    info!("Listen for socks connections @ {}", &cli.listen_addr);

    let routing_rules = read_routing_config(&cli.route_config)?;
    let router = Arc::new(RwLock::new(Router::new(routing_rules)));
    let route_cache = Arc::new(RouteCache::new());
    let command_tracker = Arc::new(CommandProcessTracker::new());

    let cleanup_task = spawn_cleanup_tracker(command_tracker.clone());

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Spawn the shutdown signal handler
    let shutdown_task = tokio::spawn(handle_shutdown_signal(
        shutdown_tx.clone(),
        command_tracker.clone(),
    ));

    handle_tcp_connections(
        listener,
        router,
        route_cache,
        command_tracker,
        shutdown_rx.clone(),
    )
    .await;
    cleanup_task.abort();
    shutdown_task.await?;

    Ok(())
}

/// Spawn periodic cleanup of the command process tracker
fn spawn_cleanup_tracker(tracker: Arc<CommandProcessTracker>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30)); // Run cleanup every 30 seconds
        loop {
            interval.tick().await;
            tracker.cleanup().await;
            debug!("Cleaned up terminated processes.");
        }
    })
}

pub async fn handle_shutdown_signal(
    shutdown_tx: watch::Sender<bool>,
    tracker: Arc<CommandProcessTracker>,
) {
    // Wait for a shutdown signal (e.g., Ctrl+C)
    signal::ctrl_c()
        .await
        .expect("Failed to listen for shutdown signal");

    println!("Shutdown signal received. Stopping all processes...");
    tracker.stop_all_processes().await;
    println!("All processes stopped. Exiting program...");

    // Send the shutdown notification to all receivers
    let _ = shutdown_tx.send(true);
}

fn spawn_and_log_error<F>(fut: F) -> task::JoinHandle<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    task::spawn(async move {
        if let Err(err) = &fut.await {
            error!("{:#}", &err)
        }
    })
}

async fn handle_tcp_connections(
    listener: TcpListener,
    router: Arc<RwLock<Router>>,
    cache: Arc<RouteCache>,
    command_tracker: Arc<CommandProcessTracker>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            Ok((socket, _client_addr)) = listener.accept() => {
                spawn_and_log_error(serve_socks5(router.clone(), cache.clone(), command_tracker.clone(), socket));
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Shutdown signal received in TCP handler. Stopping...");
                    break;
                }
            }
        }
    }
}
