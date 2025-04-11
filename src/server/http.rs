use crate::command::CommandProcessTracker;
use crate::router::router::Router;
use crate::server::utils::wait_for_port_open;
use crate::stats::ConnectionMessage;
use anyhow::bail;
use log::{debug, error, info, warn};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex};

pub async fn handle_http_connections(
    listener: TcpListener,
    router: Arc<Router>,
    mut shutdown_rx: watch::Receiver<bool>,
    command_tracker: Arc<CommandProcessTracker>,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) {
    let active_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(Vec::new()));

    loop {
        tokio::select! {
            Ok((socket, client_addr)) = listener.accept() => {
                let active_tasks_clone = active_tasks.clone();
                let router_clone = router.clone();
                let command_tracker_clone = command_tracker.clone();
                let stats_tx_clone = stats_tx.clone();

                let task_handle = tokio::spawn(async move {
                    if let Err(err) = serve_http(
                        router_clone,
                        command_tracker_clone,
                        socket,
                        stats_tx_clone,
                    ).await {
                        error!("Error handling HTTP connection: {:?}", err);
                    }

                    let mut tasks = active_tasks_clone.lock().await;
                    tasks.retain(|handle| !handle.is_finished());
                });

                active_tasks.lock().await.push(task_handle);
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Shutdown signal received in HTTP handler. Stopping...");

                    let tasks = active_tasks.lock().await.drain(..).collect::<Vec<_>>();
                    for task in tasks {
                        task.abort(); // Abort task
                        let _ = task.await; // Wait for cleanup
                    }
                    break;
                }
            }
        }
    }
}

pub async fn serve_http(
    router: Arc<Router>,
    command_tracker: Arc<CommandProcessTracker>,
    mut socket: TcpStream,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Read the HTTP request (e.g., CONNECT or GET/POST)
    let mut buffer = [0u8; 8192];
    let bytes_read = socket.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);

    if request.starts_with("CONNECT") {
        // Handle HTTP CONNECT tunneling
        let (target_host, target_port) = parse_connect_request(&request)?;

        // Notify stats
        if let Err(err) = stats_tx.try_send(ConnectionMessage::ConnectionStarted {
            host: target_host.clone(),
            port: target_port,
        }) {
            warn!("Failed to send connection stats: {}", err);
        }

        // Check if the target has a route via the router
        let upstream = router.route(&target_host).await;

        if let Some(resolved_route) = upstream {
            debug!(
                "Resolved route for {} to {}, adding to cache",
                &target_host,
                &resolved_route.upstream()
            );

            let socket_addr: SocketAddr = resolved_route.upstream().parse()?;
            let ip = socket_addr.ip().to_string();
            let port = socket_addr.port();

            if let Some(command) = resolved_route.command() {
                debug!("Handling command for route {}: {}", &target_host, command);
                crate::server::server::execute_command(command_tracker.clone(), command).await?;

                debug!("Checking if port {} on {} is open...", port, ip);
                wait_for_port_open(&ip, port).await?;
            }

            router
                .add_to_cache(target_host.clone(), resolved_route.upstream().to_string())
                .await;

            // Route exists: Use the upstream SOCKS5 proxy to connect
            let mut socks_client = fast_socks5::client::Socks5Stream::connect(
                &resolved_route.upstream().to_string(),
                target_host.clone(),
                target_port,
                fast_socks5::client::Config::default(),
            )
            .await?;

            // Notify that the tunnel is successfully established
            let response = "HTTP/1.1 200 Connection Established\r\n\r\n";
            socket.write_all(response.as_bytes()).await?;

            // Relay traffic
            tokio::io::copy_bidirectional(&mut socket, &mut socks_client).await?;
        } else {
            // No route found: Connect directly to the target
            let mut target_stream = TcpStream::connect((target_host.clone(), target_port)).await?;

            // Notify that the tunnel is successfully established
            let response = "HTTP/1.1 200 Connection Established\r\n\r\n";
            socket.write_all(response.as_bytes()).await?;

            // Relay traffic directly
            tokio::io::copy_bidirectional(&mut socket, &mut target_stream).await?;
        }
    } else {
        // Handle standard HTTP requests like GET, POST, etc.
        error!("Received unsupported HTTP request: {}", request.trim());
        let response = "HTTP/1.1 405 Method Not Allowed\r\n\r\n";
        socket.write_all(response.as_bytes()).await?;
    }

    Ok(())
}

/// Parse the CONNECT request to extract target host and port
fn parse_connect_request(request: &str) -> anyhow::Result<(String, u16)> {
    let parts: Vec<&str> = request
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Empty request"))?
        .split_whitespace()
        .collect();

    if parts.len() < 3 || parts[0] != "CONNECT" {
        bail!("Invalid CONNECT request");
    }

    let host_port = parts[1].split(':').collect::<Vec<&str>>();

    if host_port.len() != 2 {
        bail!("Invalid target in CONNECT request");
    }

    let host = host_port[0].to_string();
    let port: u16 = host_port[1].parse()?;
    Ok((host, port))
}
