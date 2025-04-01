use crate::command::CommandProcessTracker;
use crate::router::router::Router;
use crate::socks5::udp::{handle_direct_udp_connection, handle_upstream_udp_connection};
use crate::stats::ConnectionMessage;
use anyhow::bail;
use fast_socks5::server::states::CommandRead;
use fast_socks5::server::Socks5ServerProtocol;
use fast_socks5::{client, ReplyError, Socks5Command};
use log::{debug, error, info, warn};
use std::collections::HashSet;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::{watch, Mutex};
use tokio::task;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use url::Url;

// Configurable maximum duration to wait for the port to open
const PORT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const PORT_POLL_INTERVAL: Duration = Duration::from_millis(250); // Check every 250 ms
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);

fn extract_domain(url: &str) -> Option<String> {
    Url::parse(url).ok()?.domain().map(|d| d.to_string())
}

/// Spawn periodic cleanup of the command process tracker
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
    listen_addr: &str,
    route_config: &PathBuf,
    shutdown_rx: watch::Receiver<bool>,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen_addr).await?;
    info!("Listen for socks connections @ {}", listen_addr);

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

    handle_tcp_connections(listener, router, command_tracker, tcp_handler_rx, stats_tx).await;
    cleanup_task.abort();
    config_task.await?;

    Ok(())
}

async fn handle_tcp_connections(
    listener: TcpListener,
    router: Arc<Router>,
    command_tracker: Arc<CommandProcessTracker>,
    mut shutdown_rx: watch::Receiver<bool>,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) {
    let active_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(Vec::new()));

    loop {
        tokio::select! {
            Ok((socket, _client_addr)) = listener.accept() => {
                let active_tasks_clone = active_tasks.clone();

                let router = router.clone();
                let command_tracker = command_tracker.clone();
                let stats_tx = stats_tx.clone();

                let task_handle = tokio::spawn(async move {
                    if let Err(err) = serve_socks5(router, command_tracker, socket, stats_tx).await {
                        error!("Error in connection: {}", &err);
                    }

                    let mut tasks = active_tasks_clone.lock().await;
                    tasks.retain(|handle| !handle.is_finished());
                });

                active_tasks.lock().await.push(task_handle);
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Shutdown signal received in TCP handler. Stopping...");

                    let tasks = active_tasks.lock().await.drain(..).collect::<Vec<_>>();
                    for task in tasks {
                        task.abort(); // Abort task
                        let _ = task.await; // Wait for cleanup
                    }

                    command_tracker.stop_all_processes().await;
                    break;
                }
            }
        }
    }
}

pub async fn serve_socks5(
    router: Arc<Router>,
    command_tracker: Arc<CommandProcessTracker>,
    socket: tokio::net::TcpStream,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()> {
    let (proto, cmd, target_addr) = Socks5ServerProtocol::accept_no_auth(socket)
        .await?
        .read_command()
        .await?;

    if cmd != Socks5Command::TCPConnect && cmd != Socks5Command::UDPAssociate {
        proto.reply_error(&ReplyError::CommandNotSupported).await?;
        return Err(ReplyError::CommandNotSupported.into());
    }

    let (target_addr, target_port) = target_addr.into_string_and_port();
    let domain = extract_domain(&target_addr).unwrap_or(target_addr.clone());

    let upstream = if let Some(cached_route) = router.get_from_cache(&domain).await {
        debug!("Found cached route for {}: {}", &domain, &cached_route);
        Some(cached_route)
    } else {
        // Cache miss: use router and store the result in the cache
        let resolved_route = router.route(&domain).await;

        if let Some(ref resolved_route) = resolved_route {
            debug!(
                "Resolved route for {} to {}, adding to cache",
                &domain,
                &resolved_route.upstream()
            );

            if let Some(command) = resolved_route.command() {
                debug!("Handling command for route {}: {}", &domain, command);
                execute_command(command_tracker.clone(), command).await?;
            }

            router
                .add_to_cache(domain.to_string(), resolved_route.upstream().to_string())
                .await;
        }

        resolved_route.map(|r| r.upstream().to_string())
    };

    if let Err(err) = stats_tx.try_send(ConnectionMessage::ConnectionStarted {
        host: domain.clone(),
        port: target_port,
    }) {
        warn!("Failed to send connection stats: {}", err);
    };

    if let Some(upstream) = upstream {
        drop(router);

        if cmd == Socks5Command::UDPAssociate {
            handle_upstream_udp_connection(
                proto,
                &target_addr,
                target_port,
                &upstream,
                stats_tx.clone(),
            )
            .await?
        } else {
            handle_upstream_connection(proto, &target_addr, target_port, &upstream, stats_tx)
                .await?
        }
    } else {
        drop(router);
        warn!("No route for {}, connecting directly", &target_addr);

        if cmd == Socks5Command::UDPAssociate {
            handle_direct_udp_connection(proto, &target_addr, target_port, stats_tx).await?
        } else {
            handle_direct_connection(proto, &target_addr, target_port, stats_tx).await?
        }
    }

    Ok(())
}

async fn wait_for_port_open(target_addr: &str, target_port: u16) -> anyhow::Result<()> {
    let target = format!("{}:{}", target_addr, target_port);
    let start = tokio::time::Instant::now();

    loop {
        // Try to establish a connection to see if the port is open
        match TcpStream::connect(&target).await {
            Ok(_) => return Ok(()), // Port is open
            Err(_) => {
                if start.elapsed() >= PORT_WAIT_TIMEOUT {
                    return Err(anyhow::anyhow!(
                        "Timeout: Port {} on {} did not open within {:?}",
                        target_port,
                        target_addr,
                        PORT_WAIT_TIMEOUT
                    ));
                }
                // Sleep for the polling interval before retrying
                sleep(PORT_POLL_INTERVAL).await;
            }
        }
    }
}

async fn handle_upstream_connection(
    proto: Socks5ServerProtocol<TcpStream, CommandRead>,
    target_addr: &str,
    target_port: u16,
    upstream: &str,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()> {
    let socket_addr: SocketAddr = upstream.parse()?;
    let ip = socket_addr.ip().to_string();
    let port = socket_addr.port();

    debug!("Checking if port {} on {} is open...", port, ip);
    wait_for_port_open(&ip, port).await?;

    let mut config = client::Config::default();
    config.set_skip_auth(false);
    let client =
        client::Socks5Stream::connect(upstream, target_addr.to_string(), target_port, config)
            .await?;

    let inner = proto
        .reply_success(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0))
        .await?;

    transfer_data(inner, client, &target_addr, target_port, stats_tx.clone()).await?;

    Ok(())
}

async fn transfer_data<I, O>(
    mut inbound: I,
    mut outbound: O,
    ip: &str,
    port: u16,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()>
where
    I: AsyncRead + AsyncWrite + Unpin,
    O: AsyncRead + AsyncWrite + Unpin,
{
    let mut bytes_sent = 0;
    let mut bytes_received = 0;

    loop {
        let mut inbound_buf = vec![0u8; 4096];
        let mut outbound_buf = vec![0u8; 4096];

        tokio::select! {
            // Read from `inbound` and write to `outbound`, with a timeout
            inbound_result = tokio::time::timeout(TRANSFER_TIMEOUT, inbound.read(&mut inbound_buf)) => {
                let n = match inbound_result {
                    Ok(Ok(0)) => break, // EOF, exit loop
                    Ok(Ok(n)) => n,
                    Ok(Err(err)) => bail!("Inbound read error: {}, ip: {}, port: {}", err, ip, port),
                    Err(_) => bail!("Inbound read timeout, ip: {}, port: {}", ip, port),
                };
                let write_result = tokio::time::timeout(TRANSFER_TIMEOUT, outbound.write_all(&inbound_buf[..n])).await;
                if let Err(err) = write_result {
                    bail!("Outbound write error/timeout: {:?}, ip: {}, port: {}", err, ip, port);
                }
                bytes_sent += n as u64;
            }

            // Read from `outbound` and write to `inbound`, with a timeout
            outbound_result = tokio::time::timeout(TRANSFER_TIMEOUT, outbound.read(&mut outbound_buf)) => {
                let n = match outbound_result {
                    Ok(Ok(0)) => break, // EOF, exit loop
                    Ok(Ok(n)) => n,
                    Ok(Err(err)) => bail!("Outbound read error: {}, ip: {}, port: {}", err, ip, port),
                    Err(_) => bail!("Outbound read timeout, ip: {}, port: {}", ip, port),
                };
                let write_result = tokio::time::timeout(TRANSFER_TIMEOUT, inbound.write_all(&outbound_buf[..n])).await;
                if let Err(err) = write_result {
                    bail!("Inbound write error/timeout: {:?}, ip: {}, port: {}", err, ip, port);
                }
                bytes_received += n as u64;
            }
        };
    }

    debug!("Transfer complete ({}, {})", bytes_sent, bytes_received);

    // Send stats for bytes transferred
    if let Err(err) = stats_tx.try_send(ConnectionMessage::DataTransferred {
        host: ip.to_string(),
        port,
        bytes_sent,
        bytes_received,
    }) {
        warn!("Failed to send data transfer stats: {}", err);
    }

    // Notify connection end
    if let Err(err) = stats_tx.try_send(ConnectionMessage::ConnectionEnded {
        host: ip.to_string(),
        port,
    }) {
        warn!("Failed to send connection stats: {}", err);
    }

    // Cleanup sockets
    if inbound.shutdown().await.is_err() || outbound.shutdown().await.is_err() {
        warn!("Failed to properly close sockets after transfer.");
    }

    Ok(())
}

async fn handle_direct_connection(
    proto: Socks5ServerProtocol<TcpStream, CommandRead>,
    target_addr: &str,
    target_port: u16,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()> {
    let target_socket = TcpListener::bind("127.0.0.1:0").await?.local_addr()?;
    let inner = proto.reply_success(target_socket).await?;

    let target_stream = tokio::net::TcpStream::connect((target_addr, target_port)).await?;
    transfer_data(
        inner,
        target_stream,
        target_addr,
        target_port,
        stats_tx.clone(),
    )
    .await?;
    Ok(())
}

async fn execute_command(tracker: Arc<CommandProcessTracker>, command: &str) -> anyhow::Result<()> {
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
