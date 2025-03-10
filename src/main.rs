use anyhow::Result;
use clap::Parser;
use log::{error, info};
use socks_router::cli::Cli;
use socks_router::router::route_cache::RouteCache;
use socks_router::router::route_config::read_routing_config;
use socks_router::router::router::Router;
use socks_router::socks5::server::serve_socks5;
use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task;

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

    handle_tcp_connections(listener, router, route_cache).await;
    Ok(())
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
) {
    loop {
        match listener.accept().await {
            Ok((socket, _client_addr)) => {
                spawn_and_log_error(serve_socks5(router.clone(), cache.clone(), socket));
            }
            Err(err) => {
                error!("accept error = {:?}", err);
            }
        }
    }
}
