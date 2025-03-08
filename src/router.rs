use log::info;
use regex::Regex;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct Route {
    pattern: String,
    upstream: String,
}

impl Route {
    /// Matches a given input string against the route's pattern regex.
    fn matches(&self, input: &str) -> anyhow::Result<bool> {
        let regex = Regex::new(&self.pattern)?;
        Ok(regex.is_match(input))
    }

    fn upstream(&self) -> &str {
        &self.upstream
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct RoutingConfig {
    routes: Vec<Route>,
}

impl RoutingConfig {
    pub fn routes(&self) -> &Vec<Route> {
        &self.routes
    }
}

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
                return Some(rule.upstream.clone());
            }
        }
        None
    }
}

pub fn read_routing_config(file_path: &PathBuf) -> anyhow::Result<RoutingConfig> {
    // Read the file content
    let yaml_content = fs::read_to_string(file_path)?;

    // Parse the YAML into the RoutingConfig struct
    let config: RoutingConfig = serde_yaml::from_str(&yaml_content)?;

    Ok(config)
}
