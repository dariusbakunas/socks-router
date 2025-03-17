use crate::router::route_cache::RouteCache;
use crate::router::route_config::{read_routing_config, Route, RoutingConfig};
use anyhow::Context;
use anyhow::Result;
use log::{debug, error, info};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tokio::sync::{watch, RwLock};
use tokio::task;

#[derive(Debug)]
pub struct Router {
    config: Arc<RwLock<RoutingConfig>>,
    config_path: PathBuf,
    cache: Arc<RwLock<RouteCache>>,
}

impl Router {
    pub async fn new(config_path: &PathBuf) -> Result<Self> {
        let initial_config =
            read_routing_config(&config_path).context("Failed to read routing config")?;

        Ok(Self {
            config: Arc::new(RwLock::new(initial_config)),
            config_path: config_path.clone(),
            cache: Arc::new(RwLock::new(RouteCache::new())),
        })
    }

    /// Start watching the configuration file for changes
    pub async fn start_config_watcher(&self, shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        let config_path = self.config_path.clone();
        let (tx, rx) = mpsc::channel();
        let config = Arc::clone(&self.config);
        let cache = Arc::clone(&self.cache);

        // Spawn a blocking task to keep the watcher alive
        task::spawn_blocking(move || {
            // Create the watcher within the task
            let mut watcher = match RecommendedWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        let _ = tx.send(event); // Notify via the channel
                    }
                },
                Config::default(),
            ) {
                Ok(w) => w,
                Err(e) => {
                    error!("Failed to create watcher: {:?}", e);
                    return;
                }
            };

            if let Err(e) = watcher.watch(&config_path, RecursiveMode::NonRecursive) {
                error!("Failed to watch config file: {:?}", e);
                return;
            }

            // Keep task running
            debug!("Watching file changes on {:?}", config_path);
            loop {
                if *shutdown_rx.borrow() {
                    info!("Shutdown signal received. Exiting config watcher loop.");
                    break;
                }

                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(_) => {
                        info!("Config file changed! Reloading...");
                        if let Ok(new_config) = read_routing_config(&config_path) {
                            let mut config_write =
                                tokio::runtime::Handle::current().block_on(config.write());
                            *config_write = new_config;

                            let mut cache_write =
                                tokio::runtime::Handle::current().block_on(cache.write());

                            cache_write.clear();

                            info!("Configuration successfully reloaded!");
                        } else {
                            error!("Failed to reload configuration");
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Timeout: No events, but watcher is still running
                    }
                    Err(e) => {
                        error!("Channel error: {:?}", e);
                        break; // Exit the loop on channel disconnection
                    }
                }
            }
        });

        Ok(())
    }

    /// Access the configuration (read)
    pub async fn config(&self) -> RoutingConfig {
        let config = self.config.read().await;
        config.clone()
    }

    pub async fn get_from_cache(&self, key: &str) -> Option<String> {
        let cache = self.cache.read().await; // Acquire a read lock on the cache
        cache.get(key)
    }

    pub async fn add_to_cache(&self, key: String, upstream: String) {
        let mut cache = self.cache.write().await; // Acquire a write lock on the cache
        cache.insert(key, upstream); // Insert the new entry
    }

    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    pub async fn route(&self, destination: &str) -> Option<Route> {
        info!("Route {} to upstream", destination);
        let config = self.config().await;
        for rule in config.routes() {
            if rule.matches(destination) {
                return Some(rule.clone());
            }
        }
        None
    }
}
