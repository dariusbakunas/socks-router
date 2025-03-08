use regex::Regex;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct Route {
    patterns: Vec<String>,
    upstream: String,
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

pub fn read_routing_config(file_path: &PathBuf) -> anyhow::Result<RoutingConfig> {
    // Read the file content
    let yaml_content = fs::read_to_string(file_path)?;

    // Parse the YAML into the RoutingConfig struct
    let config: RoutingConfig = serde_yaml::from_str(&yaml_content)?;

    Ok(config)
}

impl Route {
    /// Matches a given input string against the route's pattern regex.
    pub fn matches(&self, input: &str) -> anyhow::Result<bool> {
        for pattern in &self.patterns {
            if let Ok(regex) = Regex::new(pattern) {
                if regex.is_match(input) {
                    return Ok(true);
                }
            } else {
                return Err(anyhow::anyhow!("Invalid regex pattern: {}", pattern));
            }
        }
        Ok(false)
    }

    pub fn upstream(&self) -> &str {
        &self.upstream
    }
}
