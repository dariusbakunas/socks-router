use log::info;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct ConnectionStats {
    // Aggregate stats per destination (host:port is the key)
    per_destination: HashMap<String, DestinationStats>,
}

#[derive(Debug, Default, Clone)]
struct DestinationStats {
    active_connection_count: u32,
    total_bytes_sent: u64,
    total_bytes_received: u64,
}

pub enum ConnectionMessage {
    ConnectionStarted {
        host: String,
        port: u16,
    },
    ConnectionEnded {
        host: String,
        port: u16,
    },
    DataTransferred {
        host: String,
        port: u16,
        bytes_sent: u64,
        bytes_received: u64,
    },
}

impl ConnectionStats {
    /// Creates a new ConnectionStats instance
    pub fn new() -> Self {
        Self {
            per_destination: HashMap::new(),
        }
    }

    /// Process a message to update the stats
    pub fn handle_message(&mut self, message: ConnectionMessage) {
        match message {
            ConnectionMessage::ConnectionStarted { host, port } => {
                let key = format!("{}:{}", host, port);
                let entry = self.per_destination.entry(key).or_default();
                entry.active_connection_count += 1;
            }
            ConnectionMessage::ConnectionEnded { host, port } => {
                let key = format!("{}:{}", host, port);
                if let Some(entry) = self.per_destination.get_mut(&key) {
                    if entry.active_connection_count > 0 {
                        entry.active_connection_count -= 1;
                    }
                }
            }
            ConnectionMessage::DataTransferred {
                host,
                port,
                bytes_sent,
                bytes_received,
            } => {
                let key = format!("{}:{}", host, port);
                let entry = self.per_destination.entry(key).or_default();
                entry.total_bytes_sent += bytes_sent;
                entry.total_bytes_received += bytes_received;
            }
        }
    }

    /// Display statistics
    pub fn print_stats(&self) {
        for (destination, stats) in &self.per_destination {
            info!(
                "Destination: {} | Active Connections: {} | Total Bytes Sent: {} | Total Bytes Received: {}",
                destination,
                stats.active_connection_count,
                stats.total_bytes_sent,
                stats.total_bytes_received
            );
        }
    }
}
