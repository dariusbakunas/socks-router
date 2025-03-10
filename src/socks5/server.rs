use crate::router::route_cache::RouteCache;
use crate::router::router::Router;
use fast_socks5::server::states::CommandRead;
use fast_socks5::server::{transfer, Socks5ServerProtocol};
use fast_socks5::{client, ReplyError, Socks5Command};
use log::{debug, warn};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use url::Url;

fn extract_domain(url: &str) -> Option<String> {
    Url::parse(url).ok()?.domain().map(|d| d.to_string())
}

pub async fn serve_socks5(
    router: Arc<RwLock<Router>>,
    cache: Arc<RouteCache>,
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
                &domain, &resolved_route
            );
            cache
                .insert(domain.to_string(), resolved_route.to_string())
                .await;
        }

        resolved_route
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

async fn handle_upstream_connection(
    proto: Socks5ServerProtocol<TcpStream, CommandRead>,
    target_addr: String,
    target_port: u16,
    upstream: &str,
) -> anyhow::Result<()> {
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
