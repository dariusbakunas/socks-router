use crate::stats::ConnectionMessage;
use anyhow::bail;
use log::{debug, warn};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn transfer_data<I, O>(
    mut inbound: I,
    mut outbound: O,
    ip: &str,
    port: u16,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()>
where
    I: AsyncRead + AsyncWrite + Unpin,
    O: AsyncRead + AsyncWrite + Unpin,
{
    let mut bytes_sent = 0;
    let mut bytes_received = 0;

    loop {
        let mut inbound_buf = vec![0u8; 4096];
        let mut outbound_buf = vec![0u8; 4096];

        tokio::select! {
            // Read from `inbound` and write to `outbound`, with a timeout
            inbound_result = tokio::time::timeout(TRANSFER_TIMEOUT, inbound.read(&mut inbound_buf)) => {
                let n = match inbound_result {
                    Ok(Ok(0)) => break, // EOF, exit loop
                    Ok(Ok(n)) => n,
                    Ok(Err(err)) => bail!("Inbound read error: {}, ip: {}, port: {}", err, ip, port),
                    Err(_) => bail!("Inbound read timeout, ip: {}, port: {}", ip, port),
                };
                let write_result = tokio::time::timeout(TRANSFER_TIMEOUT, outbound.write_all(&inbound_buf[..n])).await;
                if let Err(err) = write_result {
                    bail!("Outbound write error/timeout: {:?}, ip: {}, port: {}", err, ip, port);
                }
                bytes_sent += n as u64;
            }

            // Read from `outbound` and write to `inbound`, with a timeout
            outbound_result = tokio::time::timeout(TRANSFER_TIMEOUT, outbound.read(&mut outbound_buf)) => {
                let n = match outbound_result {
                    Ok(Ok(0)) => break, // EOF, exit loop
                    Ok(Ok(n)) => n,
                    Ok(Err(err)) => bail!("Outbound read error: {}, ip: {}, port: {}", err, ip, port),
                    Err(_) => bail!("Outbound read timeout, ip: {}, port: {}", ip, port),
                };
                let write_result = tokio::time::timeout(TRANSFER_TIMEOUT, inbound.write_all(&outbound_buf[..n])).await;
                if let Err(err) = write_result {
                    bail!("Inbound write error/timeout: {:?}, ip: {}, port: {}", err, ip, port);
                }
                bytes_received += n as u64;
            }
        };
    }

    debug!("Transfer complete ({}, {})", bytes_sent, bytes_received);

    // Send stats for bytes transferred
    if let Err(err) = stats_tx.try_send(ConnectionMessage::DataTransferred {
        host: ip.to_string(),
        port,
        bytes_sent,
        bytes_received,
    }) {
        warn!("Failed to send data transfer stats: {}", err);
    }

    // Notify connection end
    if let Err(err) = stats_tx.try_send(ConnectionMessage::ConnectionEnded {
        host: ip.to_string(),
        port,
    }) {
        warn!("Failed to send connection stats: {}", err);
    }

    // Cleanup sockets
    if inbound.shutdown().await.is_err() || outbound.shutdown().await.is_err() {
        warn!("Failed to properly close sockets after transfer.");
    }

    Ok(())
}
