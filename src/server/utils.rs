use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::sleep;

// Configurable maximum duration to wait for the port to open
const PORT_POLL_INTERVAL: Duration = Duration::from_millis(250); // Check every 250 ms
const PORT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn wait_for_port_open(target_addr: &str, target_port: u16) -> anyhow::Result<()> {
    let target = format!("{}:{}", target_addr, target_port);
    let start = tokio::time::Instant::now();

    loop {
        // Try to establish a connection to see if the port is open
        match TcpStream::connect(&target).await {
            Ok(_) => return Ok(()), // Port is open
            Err(_) => {
                if start.elapsed() >= PORT_WAIT_TIMEOUT {
                    return Err(anyhow::anyhow!(
                        "Timeout: Port {} on {} did not open within {:?}",
                        target_port,
                        target_addr,
                        PORT_WAIT_TIMEOUT
                    ));
                }
                // Sleep for the polling interval before retrying
                sleep(PORT_POLL_INTERVAL).await;
            }
        }
    }
}
