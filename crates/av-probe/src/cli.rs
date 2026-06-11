use clap::{Parser, ValueEnum};

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "Multi-machine test tool for anta-vista")]
pub struct Cli {
    /// Role in the test execution (seed or probe)
    #[arg(long, value_enum, default_value_t = Role::Probe)]
    pub role: Role,

    /// Agent ID of the seed peer to test against (optional, autodetected if omitted)
    #[arg(long)]
    pub peer: Option<String>,

    /// Timeout in seconds to wait for gossip propagation / responses
    #[arg(long, default_value_t = 10)]
    pub wait: u64,

    /// Run only a specific test (e.g. gossip_name_claim)
    #[arg(long)]
    pub test: Option<String>,

    /// Output format for test reporting
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub output: OutputFormat,

    /// Use the real MiniLM model instead of mock embeddings
    #[arg(long)]
    pub real_model: bool,

    /// Verbose output logging
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Seed,
    Probe,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Text,
}
