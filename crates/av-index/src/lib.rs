pub mod error;
pub mod filter;
pub mod index;
pub mod naming;
pub mod search;

pub use error::{IndexError, IndexResult};
pub use filter::{KindFilter, MimeFilter, QueryFilter, SchemeFilter};
pub use index::LocalIndex;
pub use naming::NameResult;
pub use search::SearchResult;
