use clap::Parser;

#[derive(Parser, Debug)]
pub struct Cli {
    #[clap(short, long)]
    pub listen_addr: String,
}
