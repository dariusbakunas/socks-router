use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version = env!("APP_VERSION"), about, long_about = None)]
pub struct Cli {
    #[clap(short, long)]
    pub listen_addr: String,

    #[arg(long, value_name = "FILE", value_parser)]
    pub route_config: PathBuf,
}
