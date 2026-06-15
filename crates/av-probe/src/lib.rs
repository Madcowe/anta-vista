pub mod cli;
pub mod output;
pub mod seed;
pub mod probe;
pub mod tests;

use clap::Parser;
use cli::{Cli, Role};
use av_net_x0x::client::X0xConfig;

pub fn run() {
    let args = Cli::parse();

    // Setup logging
    let filter = if args.verbose {
        "debug,av_probe=debug,av_net_x0x=debug"
    } else {
        "info,av_probe=info,av_net_x0x=info"
    };
    
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init();

    // Discover the x0x daemon
    let config = match X0xConfig::from_data_dir() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error: Could not discover x0x daemon config. Is x0xd running?");
            eprintln!("Details: {:?}", e);
            std::process::exit(1);
        }
    };

    match args.role {
        Role::Seed => {
            seed::run_seed(args, config);
        }
        Role::Probe => {
            probe::run_probe(args, config);
        }
    }
}
