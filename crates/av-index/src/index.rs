use std::time::{SystemTime, UNIX_EPOCH};

use av_embed::provider::{EmbeddingProvider, profile_id};
use rusqlite::Connection;

use crate::{
    error::IndexResult,
    filter::{QueryFilter, SchemeFilter},
    naming::{NameResult, lookup_name},
    search::{SearchResult, search_top_k},
};

pub struct LocalIndex<'a> {
    conn: &'a Connection,
    provider: &'a dyn EmbeddingProvider,
}

impl<'a> LocalIndex<'a> {
    pub fn new(conn: &'a Connection, provider: &'a dyn EmbeddingProvider) -> Self {
        Self { conn, provider }
    }

    /// Embed a query string and find the top-k most semantically similar resources.
    pub fn search(
        &self,
        query: &str,
        k: usize,
        filter: &QueryFilter,
    ) -> IndexResult<Vec<SearchResult>> {
        let vector = self
            .provider
            .embed_text(query)
            .map_err(|e| crate::error::IndexError::Embed(e.to_string()))?;
        let pid = profile_id(self.provider.profile());
        search_top_k(self.conn, &vector, &pid, k, filter)
    }

    /// Exact name lookup with optional scheme filter.
    pub fn resolve_name(
        &self,
        name: &str,
        scheme_filter: &SchemeFilter,
    ) -> IndexResult<Vec<NameResult>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        lookup_name(self.conn, name, scheme_filter, now)
    }
}
