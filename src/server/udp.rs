use crate::stats::ConnectionMessage;
use fast_socks5::server::states::CommandRead;
use fast_socks5::server::Socks5ServerProtocol;
use log::debug;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::net::TcpStream;

pub async fn handle_upstream_udp_connection(
    proto: Socks5ServerProtocol<TcpStream, CommandRead>,
    target_addr: &str,
    target_port: u16,
    upstream: &str,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()> {
    // Bind a local UDP socket for relaying traffic
    let local_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let bind_addr = local_socket.local_addr()?;

    // Reply to the client with the relay address
    proto.reply_success(bind_addr).await?;

    let upstream_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let upstream_addr: SocketAddr = upstream.parse()?;

    let mut buf = vec![0u8; 65535]; // Handle datagrams
    loop {
        // Receive packet from the client
        let (size, client_addr) = local_socket.recv_from(&mut buf).await?;
        let received_packet = buf[..size].to_vec();
        let (_, data) = parse_socks5_udp_packet(&received_packet[..size])?;

        // Build the SOCKS5 UDP packet for the upstream
        let udp_packet = build_socks5_udp_request(target_addr, target_port, data)?;

        // Send the packet to the upstream server
        upstream_socket.send_to(&udp_packet, upstream_addr).await?;

        // Receive response from the upstream server
        let (upstream_size, _) = upstream_socket.recv_from(&mut buf).await?;
        let response_packet = buf[..upstream_size].to_vec();

        // Forward the response back to the client
        local_socket.send_to(&response_packet, client_addr).await?;

        debug!("Transfer complete ({}, {})", data.len(), upstream_size);

        stats_tx
            .send(ConnectionMessage::DataTransferred {
                host: target_addr.to_string(),
                port: target_port,
                bytes_sent: data.len() as u64,
                bytes_received: upstream_size as u64,
            })
            .await?;
    }
}

pub async fn handle_direct_udp_connection(
    proto: Socks5ServerProtocol<TcpStream, CommandRead>,
    target_addr: &str,
    target_port: u16,
    stats_tx: tokio::sync::mpsc::Sender<ConnectionMessage>,
) -> anyhow::Result<()> {
    // Bind a local UDP socket for relaying traffic
    let local_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let bind_addr = local_socket.local_addr()?;

    // Reply to the client with the relay address
    proto.reply_success(bind_addr).await?;

    let mut buf = vec![0u8; 65535];
    loop {
        // Receive packet from the client
        let (size, client_addr) = local_socket.recv_from(&mut buf).await?;
        let received_packet = buf[..size].to_vec();
        let (target_addr_parsed, data) = parse_socks5_udp_packet(&received_packet)?;

        // Forward the packet to the target
        local_socket
            .send_to(data, (target_addr_parsed.ip(), target_addr_parsed.port()))
            .await?;

        // Receive response from the target
        let (response_size, target_addr_received) = local_socket.recv_from(&mut buf).await?;
        let relay_packet = build_socks5_udp_response(&target_addr_received, &buf[..response_size])?;

        // Send the response back to the client
        local_socket.send_to(&relay_packet, client_addr).await?;

        debug!("Transfer complete ({}, {})", data.len(), response_size);

        stats_tx
            .send(ConnectionMessage::DataTransferred {
                host: target_addr.to_string(),
                port: target_port,
                bytes_sent: data.len() as u64,
                bytes_received: response_size as u64,
            })
            .await?;
    }
}

fn build_socks5_udp_request(
    target_addr: &str,
    target_port: u16,
    payload: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let mut packet = vec![0x00, 0x00, 0x00]; // SOCKS5 UDP header: Reserved (2 bytes) + Fragment (1 byte)
    packet.push(0x03); // Address type: Domain name

    packet.push(target_addr.len() as u8);
    packet.extend_from_slice(target_addr.as_bytes());
    packet.extend_from_slice(&target_port.to_be_bytes());
    packet.extend_from_slice(payload);

    Ok(packet)
}

fn build_socks5_udp_response(client_addr: &SocketAddr, data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut response = vec![0u8; 10]; // Reserve space for the SOCKS5 UDP header

    response[3] = 1; // Address type: IPv4
    if let SocketAddr::V4(addr) = client_addr {
        response[4..8].copy_from_slice(&addr.ip().octets());
        response[8..10].copy_from_slice(&addr.port().to_be_bytes());
    } else {
        anyhow::bail!("Only IPv4 is supported for responses");
    }

    response.extend_from_slice(data);
    Ok(response)
}

fn parse_socks5_udp_packet(packet: &[u8]) -> anyhow::Result<(SocketAddr, &[u8])> {
    if packet.len() < 10 {
        anyhow::bail!("Invalid SOCKS5 UDP packet");
    }

    let address_type = packet[3];
    let target_addr = match address_type {
        1 => {
            // IPv4
            let ip = Ipv4Addr::new(packet[4], packet[5], packet[6], packet[7]);
            let port = u16::from_be_bytes([packet[8], packet[9]]);
            SocketAddr::new(IpAddr::V4(ip), port)
        }
        3 => {
            // Domain name
            let domain_len = packet[4] as usize;
            let domain = std::str::from_utf8(&packet[5..5 + domain_len])?;
            let port = u16::from_be_bytes([packet[5 + domain_len], packet[5 + domain_len + 1]]);
            format!("{}:{}", domain, port).parse()?
        }
        4 => {
            // IPv6
            let ip = Ipv6Addr::new(
                u16::from_be_bytes([packet[4], packet[5]]),
                u16::from_be_bytes([packet[6], packet[7]]),
                u16::from_be_bytes([packet[8], packet[9]]),
                u16::from_be_bytes([packet[10], packet[11]]),
                u16::from_be_bytes([packet[12], packet[13]]),
                u16::from_be_bytes([packet[14], packet[15]]),
                u16::from_be_bytes([packet[16], packet[17]]),
                u16::from_be_bytes([packet[18], packet[19]]),
            );
            let port = u16::from_be_bytes([packet[20], packet[21]]);
            SocketAddr::new(IpAddr::V6(ip), port)
        }
        _ => anyhow::bail!("Unsupported address type"),
    };

    let data = &packet[10..];
    Ok((target_addr, data))
}
