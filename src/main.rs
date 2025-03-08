use anyhow::Result;
use clap::Parser;
use fast_socks5::server::{transfer, Socks5ServerProtocol};
use fast_socks5::{client, ReplyError, Socks5Command};
use log::{error, info, warn};
use regex::Regex;
use socks_router::cli::Cli;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task;
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    spawn_socks_server().await
}

#[derive(Debug, Clone)]
struct RoutingRule {
    pattern: Regex,
    upstream_addr: String, // Address of the upstream SOCKS5 server or SSH tunnel
}

#[derive(Debug, Clone)]
struct Router {
    rules: Vec<RoutingRule>,
}

impl Router {
    fn new(rules: Vec<RoutingRule>) -> Self {
        Router { rules }
    }

    fn route(&self, destination: &str) -> Option<String> {
        info!("Route {} to upstream", destination);
        for rule in &self.rules {
            if rule.pattern.is_match(destination) {
                return Some(rule.upstream_addr.clone());
            }
        }
        None
    }
}

async fn serve_socks5(router: Arc<RwLock<Router>>, socket: tokio::net::TcpStream) -> Result<()> {
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

    let domain = extract_domain(&target_addr);

    if let Some(upstream) = router.route(domain.as_deref().unwrap_or(&target_addr)) {
        drop(router);
        let mut config = client::Config::default();
        config.set_skip_auth(false);
        let client =
            client::Socks5Stream::connect(upstream, target_addr, target_port, config).await?;

        let inner = proto
            .reply_success(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0))
            .await?;

        transfer(inner, client).await;
    } else {
        drop(router);
        warn!("No route for {}, connecting directly", &target_addr);
        // Establish a direct connection to the target
        let target_socket = TcpListener::bind("0.0.0.0:0").await?.local_addr()?;
        let inner = proto.reply_success(target_socket).await?;

        let target_stream = tokio::net::TcpStream::connect((target_addr, target_port)).await?;
        transfer(inner, target_stream).await;
    }

    Ok(())
}

fn extract_domain(url: &str) -> Option<String> {
    if let Ok(parsed) = Url::parse(url) {
        parsed.domain().map(|d| d.to_string())
    } else {
        None
    }
}

async fn spawn_socks_server() -> Result<()> {
    let cli = Cli::parse();
    let listener = TcpListener::bind(&cli.listen_addr).await?;
    info!("Listen for socks connections @ {}", &cli.listen_addr);

    let routing_rules = vec![
        RoutingRule {
            pattern: Regex::new(r"^(?:[a-zA-Z0-9-]+\.)*ifconfig\.me$").unwrap(),
            upstream_addr: "127.0.0.1:9090".into(),
        },
        RoutingRule {
            pattern: Regex::new(r"^(?:[a-zA-Z0-9-]+\.)*ipify\.org$").unwrap(),
            upstream_addr: "127.0.0.1:9091".into(),
        },
    ];

    let router = Arc::new(RwLock::new(Router::new(routing_rules)));

    // Standard TCP loop
    loop {
        match listener.accept().await {
            Ok((socket, _client_addr)) => {
                spawn_and_log_error(serve_socks5(router.clone(), socket));
            }
            Err(err) => {
                error!("accept error = {:?}", err);
            }
        }
    }
}

fn spawn_and_log_error<F>(fut: F) -> task::JoinHandle<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    task::spawn(async move {
        match fut.await {
            Ok(()) => {}
            Err(err) => error!("{:#}", &err),
        }
    })
}
