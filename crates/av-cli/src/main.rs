use av_cli::Cli;
use clap::Parser;

fn main() {
    let cli = Cli::parse();

    // Initialize logging
    let filter = match cli.verbose {
        0 => tracing::Level::WARN,
        1 => tracing::Level::INFO,
        2 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    tracing_subscriber::fmt().with_max_level(filter).init();

    let non_interactive = cli.non_interactive;

    if let Err(e) = av_cli::cmd::run(cli) {
        av_cli::output::print_error(non_interactive, &e);
        std::process::exit(1);
    }
}
