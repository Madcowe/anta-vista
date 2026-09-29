use crate::{Cli, Commands};
use thiserror::Error;

pub mod block;
pub mod index;
pub mod listen;
pub mod name;
pub mod propagate;
pub mod purge;
pub mod rate;
pub mod resolve;
pub mod search;
pub mod status;
pub mod stop;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("Daemon error: {0}")]
    Daemon(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Model error: {0}")]
    Model(String),

    #[error("Ingest error: {0}")]
    Ingest(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Validation error: {0}")]
    Validation(String),

    /// A required external dependency is missing or stopped. `code` is the
    /// stable machine-readable identifier reported as `"error"` in machine mode,
    /// so it must not change without updating callers.
    #[error("{detail}")]
    MissingDependency { code: &'static str, detail: String },

    /// Bare `av` was invoked in machine mode, where launching the interactive
    /// web interface is not allowed.
    #[error("{detail}")]
    NoSubcommand { detail: String, hint: String },

    #[error("CLI error: {0}")]
    Other(String),
}

pub type CliResult<T> = Result<T, CliError>;

pub fn run(cli: Cli) -> CliResult<()> {
    // Run startup check first
    let startup_state = crate::startup::run_startup_checks(&cli)?;

    match &cli.command {
        // Bare `av` launches the web interface, but only in interactive mode.
        // Machine mode must fail loudly rather than bind a socket and block.
        None => {
            if cli.non_interactive {
                return Err(CliError::NoSubcommand {
                    detail: "no subcommand given".to_string(),
                    hint: concat!(
                        "av launches the web interface when run without a subcommand, ",
                        "but that is disabled in machine mode. Pass a subcommand ",
                        "(see 'av --help') or drop --non-interactive to use the web interface."
                    )
                    .to_string(),
                });
            }
            crate::web::run(
                &cli,
                startup_state,
                crate::web::WebOptions {
                    host: "127.0.0.1".to_string(),
                    port: crate::web::DEFAULT_PORT,
                    open_browser: true,
                    run_for: None,
                },
                true,
            )
        }
        Some(Commands::Status) => status::run(cli.clone(), startup_state),
        Some(Commands::Resolve {
            name,
            r#type,
            scheme,
            limit,
        }) => resolve::run(
            cli.clone(),
            startup_state,
            name.clone(),
            r#type.clone(),
            scheme.clone(),
            *limit,
        ),
        Some(Commands::Search {
            query,
            scheme,
            kind,
            mime,
            limit,
            show_peers,
        }) => search::run(
            cli.clone(),
            startup_state,
            query.clone(),
            search::SearchOptions {
                scheme: scheme.clone(),
                kind: kind.clone(),
                mime: mime.clone(),
                limit: *limit,
                show_peers: *show_peers,
            },
        ),
        Some(Commands::Name {
            uri,
            name,
            r#type,
            ttl,
            no_verify,
        }) => name::run(
            cli.clone(),
            startup_state,
            uri.clone(),
            name.clone(),
            r#type.clone(),
            *ttl,
            *no_verify,
        ),
        Some(Commands::Index {
            uri,
            tags,
            no_download,
            no_verify,
            force,
        }) => index::run(
            cli.clone(),
            startup_state,
            uri.clone(),
            tags.clone(),
            *no_download,
            *no_verify,
            *force,
        ),
        Some(Commands::Rate {
            resource_id,
            rating,
            query,
        }) => rate::run(
            cli.clone(),
            startup_state,
            resource_id.clone(),
            rating.clone(),
            query.clone(),
        ),
        Some(Commands::Purge {
            resource,
            name,
            all,
            cache,
            duplicates,
            no_confirm,
        }) => purge::run(
            cli.clone(),
            startup_state,
            resource.clone(),
            name.clone(),
            *all,
            *cache,
            *duplicates,
            *no_confirm,
        ),
        Some(Commands::Listen { run_for }) => listen::run(startup_state, *run_for),
        Some(Commands::Stop { force }) => stop::run(&cli, *force),
        Some(Commands::Propagate {
            resource_id,
            location,
            description,
            mime,
        }) => propagate::run(
            cli.clone(),
            startup_state,
            resource_id.clone(),
            location.clone(),
            description.clone(),
            mime.clone(),
        ),
        Some(Commands::Block {
            resource,
            agent,
            name,
            reason,
            keep_local,
            x0x,
            list,
        }) => block::run(
            cli.clone(),
            startup_state,
            resource.clone(),
            agent.clone(),
            name.clone(),
            reason.clone(),
            *keep_local,
            *x0x,
            *list,
        ),
        Some(Commands::Unblock {
            resource,
            agent,
            name,
            all,
            x0x,
            no_confirm,
        }) => block::run_unblock(
            cli.clone(),
            startup_state,
            resource.clone(),
            agent.clone(),
            name.clone(),
            *all,
            *x0x,
            *no_confirm,
        ),
        Some(Commands::Web {
            host,
            port,
            no_open,
            run_for,
        }) => crate::web::run(
            &cli,
            startup_state,
            crate::web::WebOptions {
                host: host.clone(),
                port: *port,
                open_browser: !*no_open,
                run_for: *run_for,
            },
            false,
        ),
    }
}
