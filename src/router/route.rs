use regex::Regex;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Route {
    pattern: String,
    upstream: String,
}

impl Route {
    /// Matches a given input string against the route's pattern regex.
    pub fn matches(&self, input: &str) -> anyhow::Result<bool> {
        let regex = Regex::new(&self.pattern)?;
        Ok(regex.is_match(input))
    }

    pub fn upstream(&self) -> &str {
        &self.upstream
    }
}
