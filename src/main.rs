use std::env;

#[cfg(unix)]
use daemonize::Daemonize;

#[cfg(unix)]
use std::fs::File;

#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};

use anyhow::Result;
use clap::Parser;
use socks_router::cli::Cli;
use socks_router::server::server::spawn_socks_server;
use socks_router::stats::{ConnectionMessage, ConnectionStats};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{mpsc, watch, Mutex};

#[cfg(unix)]
fn get_temp_file_path(file_name: &str) -> PathBuf {
    let mut temp_dir = env::temp_dir(); // Get system temporary directory
    temp_dir.push(file_name); // Append the file name
    temp_dir
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    #[cfg(unix)]
    {
        if cli.daemon {
            let stdout_path = get_temp_file_path("socks_router.stdout.log");
            let stderr_path = get_temp_file_path("socks_router.stderr.log");
            let pid_path = get_temp_file_path("socks_router.pid");

            // File descriptors for logging daemon output
            let stdout = File::create(&stdout_path)?;
            let stderr = File::create(&stderr_path)?;

            let daemon = Daemonize::new()
                .pid_file(&pid_path) // Write the process PID to this file
                .stdout(stdout.try_clone()?) // Redirect stdout
                .stderr(stderr.try_clone()?); // Redirect stderr

            println!(
                "Socks-Router Daemon: \nstdout: {}\nstderr: {}\npid: {}",
                stdout_path.display(),
                stderr_path.display(),
                pid_path.display()
            );

            daemon.start().expect("Failed to daemonize process");
        }
    }

    tokio_main(cli)
}

#[tokio::main]
async fn tokio_main(cli: Cli) -> Result<()> {
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let (stx, mut srx) = mpsc::channel::<ConnectionMessage>(100);

    let stats = Arc::new(Mutex::new(ConnectionStats::new()));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let shutdown_task = tokio::spawn(handle_shutdown_signal(shutdown_tx.clone()));

    let stats_clone = Arc::clone(&stats);
    let stats_task = tokio::spawn(async move {
        while let Some(message) = srx.recv().await {
            let mut stats = stats_clone.lock().await;
            stats.handle_message(message);
        }
    });

    let stats_tx = stx.clone();

    spawn_socks_server(
        &cli.listen_addr,
        &cli.http_proxy,
        &cli.route_config,
        shutdown_rx,
        stats_tx,
    )
    .await?;

    let stats = stats.lock().await;
    stats.print_stats();

    stats_task.abort();
    shutdown_task.await?;

    Ok(())
}

pub async fn handle_shutdown_signal(shutdown_tx: watch::Sender<bool>) {
    #[cfg(unix)]
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to listen for SIGTERM");

    #[cfg(unix)]
    let sigterm_recv = sigterm.recv();

    let ctrl_c = signal::ctrl_c();

    #[cfg(unix)]
    tokio::select! {
        // Wait for Ctrl+C (cross-platform shutdown signal)
        _ = ctrl_c => {
            log::info!("Received Ctrl+C, shutting down...");
        }

        // Wait for SIGTERM (Unix-specific)
        _ = sigterm_recv => {
            log::info!("Received SIGTERM, shutting down...");
        }
    }

    #[cfg(not(unix))]
    ctrl_c.await.expect("Failed to listen for shutdown signal");

    // Send the shutdown notification to all receivers
    let _ = shutdown_tx.send(true);
}
