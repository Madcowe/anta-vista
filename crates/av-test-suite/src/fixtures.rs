//! Test fixtures: temp DBs, minimal valid resources, sample payloads

use tempfile::TempDir;

/// Temporary directory for test databases
pub struct TempDbFixture {
    dir: TempDir,
}

impl TempDbFixture {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            dir: TempDir::new()?,
        })
    }

    pub fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

impl Default for TempDbFixture {
    fn default() -> Self {
        Self::new().expect("failed to create temp dir")
    }
}

/// Sample payloads and edge cases for adversarial testing
pub mod payloads {
    /// Pathological strings
    pub const EMPTY_STRING: &str = "";
    pub const WHITESPACE_ONLY: &str = "   \t\n\r   ";
    pub const NULL_BYTE: &str = "hello\0world";
    pub const LONG_STRING: &str = "a";  // Will be repeated in tests
    pub const UNICODE_HOMOGLYPHS: &[&str] = &[
        "𝒜",  // Mathematical Alphanumeric Symbols
        "Α",  // Greek Alpha
        "А",  // Cyrillic A
    ];

    /// Magic bytes for file type detection
    pub const PDF_MAGIC: &[u8] = b"%PDF-1.4";
    pub const JPEG_MAGIC: &[u8] = b"\xFF\xD8\xFF";
    pub const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

    /// Oversized payloads near limits
    pub const MAX_PAYLOAD_BYTES: usize = 1 * 1024 * 1024; // 1 MiB
    pub const OVERSIZED_THRESHOLD: usize = MAX_PAYLOAD_BYTES + 1;
}

/// SQL injection and metacharacter strings
pub mod sql {
    pub const DROP_TABLE: &str = "'; DROP TABLE agents; --";
    pub const UNION_SELECT: &str = "' UNION SELECT * FROM agents; --";
    pub const COMMENT_BYPASS: &str = "'; -- comment";
}

/// Path traversal and naming edge cases
pub mod paths {
    pub const PARENT_DIR: &str = "..";
    pub const ABSOLUTE_PATH: &str = "/etc/passwd";
    pub const PATH_WITH_NULLS: &str = "file\0.txt";
    pub const VERY_LONG_NAME: &str = "a"; // Will be repeated to 4KB+ in tests
}
