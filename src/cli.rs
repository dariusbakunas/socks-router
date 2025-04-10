use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version = env!("APP_VERSION"), about, long_about = None)]
pub struct Cli {
    #[cfg(unix)]
    #[arg(short, long, help = "Run as a daemon process.")]
    pub daemon: bool,

    #[arg(
        short = 'l',
        long,
        default_value = "127.0.0.1:1080",
        help = "Listen address for the SOCKS server."
    )]
    pub listen_addr: String,

    #[arg(
        long,
        default_value = "127.0.0.1:8080",
        help = "Listen address for the HTTP proxy."
    )]
    pub http_proxy: String,

    #[arg(short = 'r', long, help = "Path to the routing configuration file.")]
    pub route_config: PathBuf,
}
