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
        // A missing subcommand is a usage error (clap's convention is 2), not a
        // runtime failure, so scripts can distinguish "you called me wrong" from
        // "I tried and failed".
        let code = match e {
            av_cli::cmd::CliError::NoSubcommand { .. } => 2,
            _ => 1,
        };
        std::process::exit(code);
    }
}
