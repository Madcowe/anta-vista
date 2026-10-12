pub mod cmd;
pub mod download;
pub mod listener;
pub mod network;
pub mod output;
pub mod starters;
mod starters_data;
pub mod startup;
pub mod web;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "av", version, about = "Anta-Vista CLI Client")]
pub struct Cli {
    #[arg(long, global = true, help = "Machine mode: JSON output, no prompts")]
    pub non_interactive: bool,

    #[arg(long, help = "Path to config.toml")]
    pub config: Option<PathBuf>,

    #[arg(long, default_value = "10000", help = "Network response timeout in ms")]
    pub timeout: u64,

    #[arg(long, help = "Show results progressively as they arrive")]
    pub stream: bool,

    #[arg(short, long, action = clap::ArgAction::Count, help = "Increase log verbosity")]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Commands>,
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

        #[arg(
            long,
            help = "Print the peer ids behind each network result (hidden by default; also present in --non-interactive JSON)"
        )]
        show_peers: bool,
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

        #[arg(long, help = "Remove duplicate resources sharing the same location")]
        duplicates: bool,

        #[arg(long, help = "Skip confirmation prompt (for scripting)")]
        no_confirm: bool,
    },

    #[command(about = "Listen for and respond to network queries from peers")]
    Listen {
        #[arg(long, help = "Stop after this many seconds (omit to run until Ctrl-C)")]
        run_for: Option<u64>,
    },

    #[command(about = "Stop all background av listen processes")]
    Stop {
        #[arg(long, help = "Force-kill without waiting for graceful exit")]
        force: bool,
    },

    #[command(about = "Propagate a resource into the local index by re-embedding its description")]
    Propagate {
        #[arg(help = "Resource SHA-256 hash")]
        resource_id: String,

        #[arg(help = "Resource URI/location")]
        location: String,

        #[arg(help = "Resource description text (re-embedded to create the search vector)")]
        description: String,

        #[arg(long, help = "MIME type (e.g. text/plain, text/html)")]
        mime: Option<String>,
    },

    #[command(
        about = "Block a resource, name record or peer agent locally",
        long_about = "Blocking is local policy, enforced on every path: a blocked resource is \
                      hidden from search and never served to peers, a blocked name no longer \
                      resolves, and a blocked agent's answers are dropped and excluded from search \
                      fan-out.\n\n\
                      BLOCKING A RESOURCE DELETES YOUR LOCAL COPY by default, which also removes \
                      its embedding, feedback events and relevance judgments. Pass --keep-local \
                      to block without deleting.\n\n\
                      The three kinds are independent: blocking a resource never blocks the agent \
                      that returned it."
    )]
    Block {
        #[arg(
            long,
            value_name = "RESOURCE_ID",
            help = "Block a resource SHA-256 hash"
        )]
        resource: Option<String>,

        #[arg(
            long,
            value_name = "AGENT_ID",
            help = "Block a 64-hex x0x peer agent id"
        )]
        agent: Option<String>,

        #[arg(
            long,
            value_name = "RECORD_ID",
            help = "Block a name record id (accepts 'name:<id>')"
        )]
        name: Option<String>,

        #[arg(long, help = "Record why, shown by 'av block --list'")]
        reason: Option<String>,

        #[arg(
            long,
            help = "Block the resource but keep the local copy (skips the delete)"
        )]
        keep_local: bool,

        #[arg(
            long,
            help = "Also block the agent at the x0x transport (requires the daemon; drops all \
                    of that agent's traffic on every topic, not just anta-vista results)"
        )]
        x0x: bool,

        #[arg(long, help = "List current blocks instead of adding one")]
        list: bool,
    },

    #[command(
        about = "Lift a block",
        long_about = "Removing a block does not restore data. A resource deleted when it was \
                      blocked has to be re-indexed with 'av index <uri>'."
    )]
    Unblock {
        #[arg(
            long,
            value_name = "RESOURCE_ID",
            help = "Unblock a resource SHA-256 hash"
        )]
        resource: Option<String>,

        #[arg(
            long,
            value_name = "AGENT_ID",
            help = "Unblock a 64-hex x0x peer agent id"
        )]
        agent: Option<String>,

        #[arg(
            long,
            value_name = "RECORD_ID",
            help = "Unblock a name record id (accepts 'name:<id>')"
        )]
        name: Option<String>,

        #[arg(
            long,
            help = "Lift every block. Does not restore deleted resources, and does not lift x0x \
                    transport blocks (those need a per-agent 'av unblock --agent <id> --x0x')"
        )]
        all: bool,

        #[arg(
            long,
            help = "Also lift the x0x transport block for an --agent (requires the daemon)"
        )]
        x0x: bool,

        #[arg(long, help = "Skip confirmation prompt (for scripting)")]
        no_confirm: bool,
    },

    #[command(
        name = "web",
        visible_alias = "gui",
        about = "Launch the web interface in a browser"
    )]
    Web {
        #[arg(long, default_value = "127.0.0.1", help = "Interface to bind")]
        host: String,

        #[arg(
            long,
            default_value = "8787",
            help = "Port to bind (0 picks a free port)"
        )]
        port: u16,

        #[arg(long, help = "Do not open a browser, just print the URL")]
        no_open: bool,

        #[arg(long, help = "Stop after this many seconds (omit to run until Ctrl-C)")]
        run_for: Option<u64>,
    },
}
