pub mod describe;
pub mod error;
pub mod filename;
pub mod ingest;
pub mod metadata;
pub mod mime;

pub use error::{IngestError, IngestResult};
pub use ingest::{ingest_bytes, ingest_file};
