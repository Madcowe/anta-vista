use crate::{Cli, Commands};
use thiserror::Error;

pub mod index;
pub mod listen;
pub mod name;
pub mod propagate;
pub mod purge;
pub mod rate;
pub mod resolve;
pub mod search;
pub mod status;

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

    #[error("CLI error: {0}")]
    Other(String),
}

pub type CliResult<T> = Result<T, CliError>;

pub fn run(cli: Cli) -> CliResult<()> {
    // Run startup check first
    let startup_state = crate::startup::run_startup_checks(&cli)?;

    match &cli.command {
        Commands::Status => status::run(cli.clone(), startup_state),
        Commands::Resolve {
            name,
            r#type,
            scheme,
            limit,
        } => resolve::run(
            cli.clone(),
            startup_state,
            name.clone(),
            r#type.clone(),
            scheme.clone(),
            *limit,
        ),
        Commands::Search {
            query,
            scheme,
            kind,
            mime,
            limit,
        } => search::run(
            cli.clone(),
            startup_state,
            query.clone(),
            scheme.clone(),
            kind.clone(),
            mime.clone(),
            *limit,
        ),
        Commands::Name {
            uri,
            name,
            r#type,
            ttl,
            no_verify,
        } => name::run(
            cli.clone(),
            startup_state,
            uri.clone(),
            name.clone(),
            r#type.clone(),
            *ttl,
            *no_verify,
        ),
        Commands::Index {
            uri,
            tags,
            no_download,
            no_verify,
            force,
        } => index::run(
            cli.clone(),
            startup_state,
            uri.clone(),
            tags.clone(),
            *no_download,
            *no_verify,
            *force,
        ),
        Commands::Rate {
            resource_id,
            rating,
            query,
        } => rate::run(
            cli.clone(),
            startup_state,
            resource_id.clone(),
            rating.clone(),
            query.clone(),
        ),
        Commands::Purge {
            resource,
            name,
            all,
            cache,
            duplicates,
            no_confirm,
        } => purge::run(
            cli.clone(),
            startup_state,
            resource.clone(),
            name.clone(),
            *all,
            *cache,
            *duplicates,
            *no_confirm,
        ),
        Commands::Listen { run_for } => listen::run(startup_state, *run_for),
        Commands::Propagate {
            resource_id,
            location,
            description,
            mime,
        } => propagate::run(
            cli.clone(),
            startup_state,
            resource_id.clone(),
            location.clone(),
            description.clone(),
            mime.clone(),
        ),
    }
}
