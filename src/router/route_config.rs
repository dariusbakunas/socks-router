use anyhow::Result;
use ipnetwork::IpNetwork;
use regex::Regex;
use serde::{de, Deserialize, Deserializer};
use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Deserialize, Clone)]
pub struct Matches {
    #[serde(default, with = "serde_regex")]
    regex: Option<Vec<Regex>>,
    #[serde(default, deserialize_with = "deserialize_ipnetwork_vec")]
    cidr: Option<Vec<IpNetwork>>,
}

fn deserialize_ipnetwork_vec<'de, D>(deserializer: D) -> Result<Option<Vec<IpNetwork>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<String>> = Option::deserialize(deserializer)?;
    if let Some(vec) = opt {
        let result: Result<Vec<IpNetwork>, _> =
            vec.into_iter().map(|s| IpNetwork::from_str(&s)).collect();
        result.map(Some).map_err(de::Error::custom)
    } else {
        Ok(None)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Route {
    matches: Matches,
    upstream: String,
    command: Option<String>,
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

pub fn read_routing_config(file_path: &PathBuf) -> Result<RoutingConfig> {
    // Read the file content
    let yaml_content = fs::read_to_string(file_path)?;

    // Parse the YAML into the RoutingConfig struct
    let config: RoutingConfig = serde_yaml::from_str(&yaml_content)?;

    Ok(config)
}

impl Route {
    /// Matches a given input string against the route's pattern regex or cidr.
    pub fn matches(&self, input: &str) -> bool {
        let mut matched = false;

        if let Some(regexes) = &self.matches.regex {
            matched = regexes.iter().any(|regex| regex.is_match(input));
        }

        if let Ok(ip_addr) = input.parse::<IpAddr>() {
            if let Some(cidrs) = &self.matches.cidr {
                if cidrs.iter().any(|cidr| cidr.contains(ip_addr)) {
                    matched = true;
                }
            }
        }

        matched
    }

    pub fn upstream(&self) -> &str {
        &self.upstream
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::Matches;
    use super::Route;
    use ipnetwork::IpNetwork;
    use regex::Regex;
    use std::str::FromStr;

    #[test]
    fn test_matches_with_regex_match() {
        let route = Route {
            matches: Matches {
                regex: Some(vec![Regex::new(r"^test.*").unwrap()]),
                cidr: None,
            },
            upstream: "http://example.com".to_string(),
            command: None,
        };

        assert!(route.matches("test123"));
        assert!(!route.matches("no-match"));
    }

    #[test]
    fn test_matches_with_cidr_match() {
        let route = Route {
            matches: Matches {
                regex: None,
                cidr: Some(vec![IpNetwork::from_str("192.168.1.0/24").unwrap()]),
            },
            upstream: "http://example.com".to_string(),
            command: None,
        };

        assert!(route.matches("192.168.1.42"));
        assert!(!route.matches("10.0.0.1"));
        assert!(!route.matches("invalid-ip"));
    }

    #[test]
    fn test_matches_with_both_regex_and_cidr() {
        let route = Route {
            matches: Matches {
                regex: Some(vec![Regex::new(r"^test.*").unwrap()]),
                cidr: Some(vec![IpNetwork::from_str("192.168.1.0/24").unwrap()]),
            },
            upstream: "http://example.com".to_string(),
            command: None,
        };

        assert!(route.matches("test123")); // Matches regex
        assert!(route.matches("192.168.1.42")); // Matches CIDR
        assert!(!route.matches("no-match"));
        assert!(!route.matches("10.0.0.1")); // Not in CIDR and does not match regex
    }

    #[test]
    fn test_matches_with_no_regex_and_no_cidr() {
        let route = Route {
            matches: Matches {
                regex: None,
                cidr: None,
            },
            upstream: "http://example.com".to_string(),
            command: None,
        };

        assert!(!route.matches("anything"));
        assert!(!route.matches("192.168.1.42"));
    }

    #[test]
    fn test_matches_with_invalid_ip() {
        let route = Route {
            matches: Matches {
                regex: None,
                cidr: Some(vec![IpNetwork::from_str("10.0.0.0/8").unwrap()]),
            },
            upstream: "http://example.com".to_string(),
            command: None,
        };

        assert!(!route.matches("not-an-ip")); // Invalid IP address should not match
    }
}
