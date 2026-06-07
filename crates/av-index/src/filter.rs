use av_core::types::ResourceKind;

/// Restrict results to these URI schemes (lowercase, e.g. "ant", "https").
/// Empty = allow all.
#[derive(Debug, Clone, Default)]
pub struct SchemeFilter {
    pub allowed: Vec<String>,
}

impl SchemeFilter {
    pub fn new(schemes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: schemes
                .into_iter()
                .map(|s| s.into().to_lowercase())
                .collect(),
        }
    }

    /// Returns true if this scheme is permitted (or filter is empty = allow all).
    pub fn allows(&self, scheme: Option<&str>) -> bool {
        if self.allowed.is_empty() {
            return true;
        }
        match scheme {
            Some(s) => self.allowed.contains(&s.to_lowercase()),
            None => false,
        }
    }
}

/// Filter by ResourceKind.
#[derive(Debug, Clone, Default)]
pub struct KindFilter {
    pub allowed: Vec<ResourceKind>,
}

impl KindFilter {
    pub fn new(kinds: impl IntoIterator<Item = ResourceKind>) -> Self {
        Self {
            allowed: kinds.into_iter().collect(),
        }
    }

    pub fn allows(&self, kind: &ResourceKind) -> bool {
        if self.allowed.is_empty() {
            return true;
        }
        // Compare by discriminant (Other(_) matches any Other)
        self.allowed
            .iter()
            .any(|k| std::mem::discriminant(k) == std::mem::discriminant(kind))
    }
}

/// Filter by MIME type prefix (e.g. "image/" matches image/jpeg, image/png, ...).
#[derive(Debug, Clone, Default)]
pub struct MimeFilter {
    pub prefixes: Vec<String>,
}

impl MimeFilter {
    pub fn new(prefixes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            prefixes: prefixes
                .into_iter()
                .map(|s| s.into().to_lowercase())
                .collect(),
        }
    }

    pub fn allows(&self, mime: &str) -> bool {
        if self.prefixes.is_empty() {
            return true;
        }
        let lower = mime.to_lowercase();
        self.prefixes.iter().any(|p| lower.starts_with(p.as_str()))
    }
}

/// Combined query filter.
#[derive(Debug, Clone, Default)]
pub struct QueryFilter {
    pub scheme: SchemeFilter,
    pub kind: KindFilter,
    pub mime: MimeFilter,
}
