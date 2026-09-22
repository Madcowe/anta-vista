pub mod describe;
pub mod error;
pub mod filename;
pub mod ingest;
pub mod location;
pub mod metadata;
pub mod mime;
pub mod watchlist;

pub use error::{IngestError, IngestResult};
pub use ingest::{ingest_bytes, ingest_file};
pub use watchlist::apply_tags;
