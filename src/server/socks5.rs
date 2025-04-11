use crate::command::CommandProcessTracker;
use crate::router::Router;
use crate::server::execute_command;
use crate::server::transfer_data::transfer_data;
use crate::server::udp::{handle_direct_udp_connection, handle_upstream_udp_connection};
use crate::server::utils::wait_for_port_open;
use crate::stats::ConnectionMessage;
use fast_socks5::server::states::CommandRead;
use fast_socks5::server::Socks5ServerProtocol;
use fast_socks5::{client, ReplyError, Socks5Command};
use log::{debug, error, info, warn};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex};
use url::Url;

pub async fn handle_tcp_connections(
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

pub(crate) async fn handle_upstream_connection(
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

    transfer_data(inner, client, target_addr, target_port, stats_tx.clone()).await?;

    Ok(())
}

pub(crate) async fn handle_direct_connection(
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

fn extract_domain(url: &str) -> Option<String> {
    Url::parse(url).ok()?.domain().map(|d| d.to_string())
}
