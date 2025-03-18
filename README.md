# Socks Router

Socks Router is a customizable, asynchronous SOCKS5 proxy server with route-based traffic forwarding. It allows users to define routing rules using regular expressions and forward traffic to different upstream SOCKS5 proxies or directly to their destinations.

## Features

- **Custom Routing Rules**: Define rules in a YAML file to route traffic based on domain patterns.
- **Asynchronous Performance**: Built with `tokio` for high-performance network handling.
- **Flexible Upstream Proxying**: Route traffic to specific upstream SOCKS5 servers or fallback to direct connections for unmatched routes.
- **Automatic configuration reloading**: Changes to routing configuration are automatically reloaded during runtime

## Installation

1. Clone the repository:
   ```bash
   git clone <repository-url>
   cd socks-router
   ```

2. Build the project using Cargo:
   ```bash
   cargo build --release
   ```

3. Run the application with your configuration:
   ```bash
   ./target/release/socks-router --listen-addr 127.0.0.1:1080 --route-config example-routes.yaml
   ```

## Configuration

Create a routing configuration file (e.g., `example-routes.yaml`) to define your traffic routing rules. For example:

```yaml
routes:
   - upstream: "127.0.0.1:9091"
     command: "ssh -N -D9091 ubuntu@X.X.X.X"
     matches:
        regex:
           - "^(?:[a-zA-Z0-9-]+\\.)*ifconfig\\.me$"
   - upstream: "127.0.0.1:9090"
     matches:
        regex:
           - "^(?:[a-zA-Z0-9-]+\\.)*ipify\\.org$"
           - "icanhazip.com"
        cidr:
           - 10.0.0.0/24
```

- **`matches.regex`**: A list of regular expressions defining the domains that match this route.
- **`matches.cidr`**: A list of IP CIDRs defining IP ranges that match this route.
- **`upstream`**: The SOCKS5 server address to forward matching traffic to.
- **`command`**: Command to execute before establishing the tunnel (example: `ssh -N -D9091 ubuntu@X.X.X.X`)

### Command-line Options

- `--listen-addr <addr>`: Address and port on which the SOCKS proxy listens (default: `127.0.0.1:1080`).
- `--route-config <file>`: Path to the YAML file containing the routing rules.
- `--daemon`: (Unix Only) Run it as a daemon process

## How It Works

1. **Routing Rules**: Incoming SOCKS5 requests are matched against the configured rules using domain name patterns.
2. **Custom Routes**: Matched traffic is forwarded to the specified upstream SOCKS5 proxy.
3. **Fallback**: Traffic that does not match any rule is connected to its destination directly.

## Example Usage

Run the SOCKS Router on `127.0.0.1:1080` with the `example-routes.yaml` configuration file:

```bash
cargo run -- --listen-addr 127.0.0.1:1080 --route-config example-routes.yaml
```

## Dependencies

- [Tokio](https://tokio.rs/) for asynchronous networking.
- [Regex](https://docs.rs/regex/) for domain pattern matching.
- [serde_yaml](https://docs.rs/serde_yaml/) for parsing YAML configurations.
- [fast-socks5](https://docs.rs/fast-socks5/) for SOCKS5 proxy support.

## License

This project is licensed under the MIT License. See the `LICENSE` file for details.

---

Happy routing! 🚀