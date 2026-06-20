pub mod cmd;
pub mod download;
pub mod network;
pub mod output;
pub mod startup;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "av", version, about = "Anta-Vista CLI Client")]
pub struct Cli {
    #[arg(long, help = "Machine mode: JSON output, no prompts")]
    pub non_interactive: bool,

    #[arg(long, help = "Path to config.toml")]
    pub config: Option<PathBuf>,

    #[arg(long, default_value = "5000", help = "Network response timeout in ms")]
    pub timeout: u64,

    #[arg(long, help = "Show results progressively as they arrive")]
    pub stream: bool,

    #[arg(short, long, action = clap::ArgAction::Count, help = "Increase log verbosity")]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    #[command(about = "Show daemon and model health status")]
    Status,

    #[command(about = "Resolve a name to URI(s) via DNS-like lookup")]
    Resolve {
        #[arg(help = "The name to resolve")]
        name: String,

        #[arg(long, value_parser = ["a", "txt", "uri", "service"], default_value = "uri", help = "Record type filter")]
        r#type: String,

        #[arg(long, help = "Filter results by target scheme (e.g. ant, https)")]
        scheme: Option<String>,

        #[arg(long, default_value = "10", help = "Max results")]
        limit: usize,
    },

    #[command(about = "Semantic search for resources")]
    Search {
        #[arg(help = "The semantic query")]
        query: String,

        #[arg(long, help = "Filter by URI scheme")]
        scheme: Option<String>,

        #[arg(long, help = "Filter by resource kind: text, image, audio, file, pdf")]
        kind: Option<String>,

        #[arg(long, help = "Filter by MIME type prefix")]
        mime: Option<String>,

        #[arg(long, default_value = "10", help = "Max results")]
        limit: usize,
    },

    #[command(about = "Register a name → URI mapping")]
    Name {
        #[arg(help = "Target URI")]
        uri: String,

        #[arg(help = "Name to register")]
        name: String,

        #[arg(long, value_parser = ["a", "txt", "uri", "service"], default_value = "uri", help = "Record type")]
        r#type: String,

        #[arg(long, default_value = "3600", help = "TTL in seconds")]
        ttl: u32,

        #[arg(long, help = "Skip URI reachability check")]
        no_verify: bool,
    },

    #[command(about = "Ingest and index a URI for search")]
    Index {
        #[arg(help = "URI to index")]
        uri: String,

        #[arg(long, help = "Comma-separated tags to add to description")]
        tags: Option<String>,

        #[arg(long, help = "Skip downloading content (use URI metadata only)")]
        no_download: bool,

        #[arg(long, help = "Skip URI reachability check")]
        no_verify: bool,

        #[arg(long, help = "Re-index even if resource already exists")]
        force: bool,
    },

    #[command(about = "Submit feedback rating for a resource")]
    Rate {
        #[arg(help = "Resource SHA-256 hash")]
        resource_id: String,

        #[arg(value_parser = ["useful", "not-useful", "incorrect", "high-confidence"], help = "Rating classification")]
        rating: String,

        #[arg(long, help = "Original query context (improves feedback quality)")]
        query: Option<String>,
    },

    #[command(about = "Clear local database entries and broadcast cache")]
    Purge {
        #[arg(long, help = "Delete a specific resource and its embeddings")]
        resource: Option<String>,

        #[arg(long, help = "Delete name records for a specific name")]
        name: Option<String>,

        #[arg(long, help = "Clear entire local database (with confirmation)")]
        all: bool,

        #[arg(long, help = "Clear only the query cache")]
        cache: bool,

        #[arg(long, help = "Skip confirmation prompt (for scripting)")]
        no_confirm: bool,
    },
}
