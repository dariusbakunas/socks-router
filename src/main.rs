use anyhow::Result;
use clap::Parser;
use fast_socks5::server::{transfer, Socks5ServerProtocol};
use fast_socks5::{client, ReplyError, Socks5Command};
use log::{error, info, warn};
use regex::Regex;
use serde::Deserialize;
use socks_router::cli::Cli;
use std::fs;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task;
use url::Url;

#[derive(Debug, Deserialize, Clone)]
struct Route {
    pattern: String,
    upstream: String,
}

impl Route {
    /// Matches a given input string against the route's pattern regex.
    fn matches(&self, input: &str) -> Result<bool> {
        let regex = Regex::new(&self.pattern)?;
        Ok(regex.is_match(input))
    }

    fn upstream(&self) -> &str {
        &self.upstream
    }
}

#[derive(Debug, Deserialize, Clone)]
struct RoutingConfig {
    routes: Vec<Route>,
}

impl RoutingConfig {
    pub fn routes(&self) -> &Vec<Route> {
        &self.routes
    }
}

fn read_routing_config(file_path: &PathBuf) -> Result<RoutingConfig> {
    // Read the file content
    let yaml_content = fs::read_to_string(file_path)?;

    // Parse the YAML into the RoutingConfig struct
    let config: RoutingConfig = serde_yaml::from_str(&yaml_content)?;

    Ok(config)
}

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
    config: RoutingConfig,
}

impl Router {
    fn new(config: RoutingConfig) -> Self {
        Router { config }
    }

    fn route(&self, destination: &str) -> Option<String> {
        info!("Route {} to upstream", destination);
        for rule in self.config.routes() {
            if rule.matches(destination).unwrap_or(false) {
                return Some(rule.upstream.clone());
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

    let routing_rules = read_routing_config(&cli.route_config)?;

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
