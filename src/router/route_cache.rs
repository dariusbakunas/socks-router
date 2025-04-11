use std::collections::HashMap;

#[derive(Debug)]
pub struct RouteCache {
    cache: HashMap<String, String>,
}

impl Default for RouteCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Attempts to get a domain route from the cache.
    pub fn get(&self, domain: &str) -> Option<String> {
        self.cache.get(domain).cloned()
    }

    /// Adds a domain-to-route mapping to the cache.
    pub fn insert(&mut self, domain: String, route: String) {
        self.cache.insert(domain, route);
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}
