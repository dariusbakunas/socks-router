use crate::command::CommandProcessTracker;
use crate::router::route_cache::RouteCache;
use crate::router::router::Router;
use fast_socks5::server::states::CommandRead;
use fast_socks5::server::{transfer, Socks5ServerProtocol};
use fast_socks5::{client, ReplyError, Socks5Command};
use log::{debug, warn};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::sleep;
use url::Url;

// Configurable maximum duration to wait for the port to open
const PORT_WAIT_TIMEOUT: Duration = Duration::from_secs(10); // e.g., maximum 10 seconds
const PORT_POLL_INTERVAL: Duration = Duration::from_millis(250); // Check every 250 ms

fn extract_domain(url: &str) -> Option<String> {
    Url::parse(url).ok()?.domain().map(|d| d.to_string())
}

pub async fn serve_socks5(
    router: Arc<RwLock<Router>>,
    cache: Arc<RouteCache>,
    command_tracker: Arc<CommandProcessTracker>,
    socket: tokio::net::TcpStream,
) -> anyhow::Result<()> {
    let router = router.read().await;
    let (proto, cmd, target_addr) = Socks5ServerProtocol::accept_no_auth(socket)
        .await?
        .read_command()
        .await?;

    if cmd != Socks5Command::TCPConnect {
        proto.reply_error(&ReplyError::CommandNotSupported).await?;
        return Err(ReplyError::CommandNotSupported.into());
    }

    let (target_addr, target_port) = target_addr.into_string_and_port();
    let domain = extract_domain(&target_addr).unwrap_or(target_addr.clone());

    let upstream = if let Some(cached_route) = cache.get(&domain).await {
        debug!("Found cached route for {}: {}", &domain, &cached_route);
        Some(cached_route)
    } else {
        // Cache miss: use router and store the result in the cache
        let resolved_route = router.route(&domain);

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

            cache
                .insert(domain.to_string(), resolved_route.upstream().to_string())
                .await;
        }

        resolved_route.map(|r| r.upstream().to_string())
    };

    if let Some(upstream) = upstream {
        drop(router);
        handle_upstream_connection(proto, target_addr, target_port, &upstream).await?
    } else {
        drop(router);
        warn!("No route for {}, connecting directly", &target_addr);
        handle_direct_connection(proto, target_addr, target_port).await?
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
    target_addr: String,
    target_port: u16,
    upstream: &str,
) -> anyhow::Result<()> {
    let socket_addr: SocketAddr = upstream.parse()?;
    let ip = socket_addr.ip().to_string();
    let port = socket_addr.port();

    debug!("Checking if port {} on {} is open...", port, ip);
    wait_for_port_open(&ip, port).await?;

    let mut config = client::Config::default();
    config.set_skip_auth(false);
    let client = client::Socks5Stream::connect(upstream, target_addr, target_port, config).await?;

    let inner = proto
        .reply_success(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0))
        .await?;

    transfer(inner, client).await;
    Ok(())
}

async fn handle_direct_connection(
    proto: Socks5ServerProtocol<TcpStream, CommandRead>,
    target_addr: String,
    target_port: u16,
) -> anyhow::Result<()> {
    let target_socket = TcpListener::bind("0.0.0.0:0").await?.local_addr()?;
    let inner = proto.reply_success(target_socket).await?;

    let target_stream = tokio::net::TcpStream::connect((target_addr, target_port)).await?;
    transfer(inner, target_stream).await;
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

    // Track the process
    tracker.add_process(command.to_string(), child).await;

    Ok(())
}
