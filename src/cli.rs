use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct Cli {
    #[clap(short, long)]
    pub listen_addr: String,

    #[arg(long, value_name = "FILE", value_parser)]
    pub route_config: PathBuf,
}
