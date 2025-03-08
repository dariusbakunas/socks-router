use crate::router::route::Route;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct RoutingConfig {
    routes: Vec<Route>,
}

impl RoutingConfig {
    pub fn routes(&self) -> &Vec<Route> {
        &self.routes
    }
}

pub fn read_routing_config(file_path: &PathBuf) -> anyhow::Result<RoutingConfig> {
    // Read the file content
    let yaml_content = fs::read_to_string(file_path)?;

    // Parse the YAML into the RoutingConfig struct
    let config: RoutingConfig = serde_yaml::from_str(&yaml_content)?;

    Ok(config)
}
