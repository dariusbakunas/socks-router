use crate::router::route_config::RoutingConfig;
use log::info;

#[derive(Debug, Clone)]
pub struct Router {
    config: RoutingConfig,
}

impl Router {
    pub fn new(config: RoutingConfig) -> Self {
        Router { config }
    }

    pub fn route(&self, destination: &str) -> Option<String> {
        info!("Route {} to upstream", destination);
        for rule in self.config.routes() {
            if rule.matches(destination).unwrap_or(false) {
                return Some(rule.upstream().to_string());
            }
        }
        None
    }
}
