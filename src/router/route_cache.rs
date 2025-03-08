use std::collections::HashMap;
use tokio::sync::RwLock;

pub struct RouteCache {
    cache: RwLock<HashMap<String, String>>,
}

impl RouteCache {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Attempts to get a domain route from the cache.
    pub async fn get(&self, domain: &str) -> Option<String> {
        let cache = self.cache.read().await;
        cache.get(domain).cloned()
    }

    /// Adds a domain-to-route mapping to the cache.
    pub async fn insert(&self, domain: String, route: String) {
        let mut cache = self.cache.write().await;
        cache.insert(domain, route);
    }
}
