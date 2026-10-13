use crate::{filename::tokenize_filename_opt, metadata::ExtractedMeta};
use av_core::constants::{ANT_MANIFEST_MIME, EMBED_WINDOW_CHARS, WATCHLIST_MIME};

/// Semantic label for a MIME type, used in description synthesis.
/// Returns (with_filename_label, no_filename_fallback).
pub fn semantic_label(mime: &str) -> (&'static str, &'static str) {
    match mime {
        "application/x-watch-list" => ("watch list", "media watch list"),
        ANT_MANIFEST_MIME => ("file manifest", "file manifest"),
        "image/jpeg" | "image/jpg" => ("image", "photograph or image"),
        "image/png" => ("image", "image"),
        "image/gif" => ("animated image or gif", "animated image or gif"),
        "image/webp" => ("image", "image"),
        "audio/mpeg" | "audio/mp3" => ("music audio", "music or audio"),
        "audio/flac" => ("audio", "audio"),
        "audio/ogg" => ("audio", "audio"),
        "audio/wav" => ("audio", "audio"),
        "video/mp4" => ("video", "video"),
        "video/webm" => ("video", "video"),
        "application/pdf" => ("document", "document or report"),
        "text/plain" => ("text document", "text document"),
        "text/html" => ("web page", "web page"),
        "text/markdown" => ("markdown document", "markdown document"),
        "text/csv" => ("data file", "data file"),
        "application/json" => ("data file", "data file"),
        _ => {
            // Fallback by major type
            match mime.split('/').next().unwrap_or("") {
                "image" => ("image", "image"),
                "audio" => ("audio", "audio"),
                "video" => ("video", "video"),
                "text" => ("text document", "text document"),
                _ => ("file", "file"),
            }
        }
    }
}

/// Base description for a watch‑list bundle (no items, no tags).
/// Only the last path segment's stem is used (e.g. "Public Domain" from
/// ".../catalog/Public Domain.watch-list") so host/directory tokens don't eat
/// into the embedding window that is reserved for media item names.
pub fn watchlist_base(mime: &str, filename: Option<&str>, meta: &ExtractedMeta) -> String {
    let (_, no_fn_label) = semantic_label(mime);
    let stem = filename.and_then(|f| {
        std::path::Path::new(f)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
    });
    let fn_tokens = stem.as_deref().and_then(tokenize_filename_opt);
    match &fn_tokens {
        Some(tokens) => format!("a {} {}", tokens, no_fn_label),
        None => {
            if let Some(title) = &meta.title {
                let clean = title.trim();
                if !clean.is_empty() {
                    return format!("a {} titled \"{}\"", no_fn_label, clean);
                }
            }
            format!("a {} file", no_fn_label)
        }
    }
}

/// Base description for an Autonomi file manifest (`.ant`): the bare type,
/// qualified by the name the creator gave it or the name the file was saved
/// under, in that order.
///
/// The manifest's own `name` outranks the filename deliberately: it is the
/// label its creator chose (a suggested root directory name), while the
/// filename of a record fetched from the network is often an address-derived
/// `….ant` that says nothing. Mirrors `watchlist_base`'s shape otherwise —
/// stem tokens only, so a URL's directories cannot eat the embed window.
pub fn manifest_base(filename: Option<&str>, meta: &ExtractedMeta) -> String {
    let mut base = if let Some(title) = meta
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        format!("an autonomi file manifest titled \"{title}\"")
    } else {
        let stem = filename.and_then(|f| {
            std::path::Path::new(f)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        });
        match stem.as_deref().and_then(tokenize_filename_opt) {
            Some(tokens) => format!("an autonomi file manifest for {tokens}"),
            None => "an autonomi file manifest".to_string(),
        }
    };
    // A record that stored only the first batch of entries says the true
    // total here, because `synthesize` then lists no paths at all: a partial
    // listing whose tail counts the stored prefix would miscount the file.
    if let Some(count) = manifest_stored_incomplete(meta) {
        base = format!("{base} with {count} entries");
    }
    base
}

/// The true entry count when the stored record holds only a prefix of the
/// manifest's entries, else `None`.
fn manifest_stored_incomplete(meta: &ExtractedMeta) -> Option<usize> {
    let manifest = meta.extra.get("manifest")?;
    let count = manifest.get("entry_count")?.as_u64()? as usize;
    let stored = manifest.get("entries")?.as_array()?.len();
    Some(count).filter(|count| count > &stored)
}

/// Every stored entry's path, in manifest order (entries without a path
/// extract under their content address and have nothing to show).
fn manifest_paths(meta: &ExtractedMeta) -> Vec<String> {
    meta.extra
        .get("manifest")
        .and_then(|m| m.get("entries"))
        .and_then(|e| e.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("path").and_then(|p| p.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Fill a description into the embedding window: base first, then optional
/// tags, then as many item names as fit. Items are the expendable tail.
/// Returned string is guaranteed to have at most `budget` characters.
pub fn fill_into_window(
    base: &str,
    items: &[String],
    tags: Option<&[String]>,
    budget: usize,
) -> String {
    let mut head = base.trim().to_string();
    if let Some(tag_list) = tags {
        if !tag_list.is_empty() {
            let tag_str = tag_list.join(", ");
            head = format!("{} tagged as: {}", head, tag_str);
        }
    }

    if items.is_empty() {
        return clamp_to_budget(head, budget);
    }

    // Fast path: everything fits.
    let full_items = items.join(", ");
    let candidate = format!("{} containing: {}", head, full_items);
    if candidate.chars().count() <= budget {
        return candidate;
    }

    // Greedy fill: place items while the tail marker still fits.
    let mut placed = head.clone();
    placed.push_str(" containing: ");
    let mut placed_count = 0usize;
    let items_len = items.len();
    for item in items {
        let candidate = if placed_count == 0 {
            format!("{}{}", placed, item)
        } else {
            format!("{}, {}", placed, item)
        };
        let more = items_len - placed_count - 1;
        let tail = tail_marker(more, items_len);
        if candidate.chars().count() + tail.chars().count() <= budget {
            placed = candidate;
            placed_count += 1;
        } else {
            break;
        }
    }

    let tail = tail_marker(items_len - placed_count, items_len);
    clamp_to_budget(format!("{}{}", placed, tail), budget)
}

fn tail_marker(more: usize, total: usize) -> String {
    if more == 0 {
        String::new()
    } else {
        format!(" and {} more of {}", more, total)
    }
}

fn clamp_to_budget(s: String, budget: usize) -> String {
    let len = s.chars().count();
    if len <= budget {
        return s;
    }
    // Hard truncate, preserving as much as possible.
    let mut truncated: String = s.chars().take(budget.saturating_sub(4)).collect();
    truncated.push_str(" ...");
    truncated
}

/// Truncate text at a sentence boundary, at most `max_chars` characters.
fn truncate_text(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    let search_end = max_chars.saturating_sub(10).min(truncated.len());
    if let Some(pos) = truncated[..search_end].rfind(". ") {
        format!("{}...", &truncated[..=pos])
    } else if let Some(pos) = truncated[..search_end].rfind("\n\n") {
        format!("{}...", &truncated[..pos])
    } else if let Some(pos) = truncated[..search_end].rfind('!') {
        format!("{}...", &truncated[..=pos])
    } else if let Some(pos) = truncated[..search_end].rfind('?') {
        format!("{}...", &truncated[..=pos])
    } else {
        format!("{}...", truncated)
    }
}

/// Synthesize a canonical natural-language description for a resource.
pub fn synthesize(mime: &str, filename: Option<&str>, meta: &ExtractedMeta) -> String {
    if mime == WATCHLIST_MIME {
        use crate::watchlist::{base_from_meta, items_from_meta};
        let base = base_from_meta(meta).unwrap_or_else(|| watchlist_base(mime, filename, meta));
        let items = crate::series::compact_items(&items_from_meta(meta));
        return fill_into_window(&base, &items, None, EMBED_WINDOW_CHARS);
    }

    if mime == ANT_MANIFEST_MIME {
        // A manifest describes itself by listing: base first (which carries
        // the true total when only a prefix of entries was stored), then
        // every path we can honestly claim to hold.
        let base = manifest_base(filename, meta);
        let items = if manifest_stored_incomplete(meta).is_some() {
            Vec::new()
        } else {
            manifest_paths(meta)
        };
        return fill_into_window(&base, &items, None, EMBED_WINDOW_CHARS);
    }

    let subtype = mime.split('/').nth(1).unwrap_or("unknown");
    let (with_fn_label, no_fn_label) = semantic_label(mime);
    let major = mime.split('/').next().unwrap_or("");

    // For text-based documents with content preview, use it
    if let Some(preview) = &meta.content_preview {
        if major == "text" || mime == "application/pdf" {
            let excerpt = truncate_text(preview, 800);
            if !excerpt.is_empty() {
                let fn_tokens = filename.and_then(|f| tokenize_filename_opt(f));
                if let Some(ctx) = &fn_tokens {
                    return format!("a {} from {}: {}", no_fn_label, ctx, excerpt);
                }
                if let Some(title) = &meta.title {
                    let clean = title.trim();
                    if !clean.is_empty() {
                        return format!("a {} titled \"{}\": {}", no_fn_label, clean, excerpt);
                    }
                }
                return format!("a {}: {}", no_fn_label, excerpt);
            }
        }
    }

    // Fall through to existing filename/title logic for non-text content
    let fn_tokens = filename.and_then(|f| tokenize_filename_opt(f));
    match &fn_tokens {
        Some(tokens) => {
            if major == "audio" {
                format!("a {} {} file", tokens, with_fn_label)
            } else {
                format!("a {} {} file in {} format", tokens, with_fn_label, subtype)
            }
        }
        None => {
            if let Some(title) = &meta.title {
                let clean = title.trim();
                if !clean.is_empty() {
                    return format!("a {} titled \"{}\"", no_fn_label, clean);
                }
            }
            format!("a {} file", no_fn_label)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use av_core::constants::WATCHLIST_MIME;
    use serde_json::json;

    fn manifest_meta(
        name: Option<&str>,
        entry_count: usize,
        entries: serde_json::Value,
    ) -> ExtractedMeta {
        ExtractedMeta {
            title: name.map(str::to_owned),
            extra: json!({
                "manifest": { "name": name, "entry_count": entry_count, "entries": entries }
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_manifest_describes_itself_by_listing_its_entries() {
        let meta = manifest_meta(
            Some("site"),
            2,
            json!([
                {"path": "index.html", "size": 120, "source": "public"},
                {"path": "style.css", "size": 34, "source": "embedded"},
            ]),
        );
        // The creator's own name outranks the address-derived filename.
        let description = synthesize(ANT_MANIFEST_MIME, Some("711c7e20.ant"), &meta);
        assert_eq!(
            description,
            "an autonomi file manifest titled \"site\" containing: index.html, style.css"
        );
    }

    #[test]
    fn a_manifest_without_a_name_falls_back_to_its_filename_then_the_bare_type() {
        let meta = manifest_meta(None, 0, json!([]));
        assert_eq!(
            synthesize(ANT_MANIFEST_MIME, Some("photo-gallery.ant"), &meta),
            "an autonomi file manifest for photo gallery"
        );
        assert_eq!(
            synthesize(ANT_MANIFEST_MIME, None, &meta),
            "an autonomi file manifest"
        );
    }

    #[test]
    fn an_entry_without_a_path_contributes_nothing_to_the_listing() {
        // Entries without a path are extracted under their content address and
        // carry nothing worth showing; the ones that do keep their order.
        let meta = manifest_meta(
            Some("site"),
            2,
            json!([
                {"path": null, "size": 3, "source": "embedded"},
                {"path": "b.txt", "size": 1, "source": "public"},
            ]),
        );
        assert_eq!(
            synthesize(ANT_MANIFEST_MIME, None, &meta),
            "an autonomi file manifest titled \"site\" containing: b.txt"
        );
    }

    #[test]
    fn a_manifest_that_stored_only_a_prefix_of_its_entries_counts_them_instead_of_listing() {
        // The honesty rule: `metadata_json` keeps the first 500 entries, so
        // when only a prefix is stored the description states the true total
        // and lists nothing — a partial listing's fill tail would count the
        // stored prefix as though it were the whole manifest.
        let meta = manifest_meta(
            Some("big"),
            100_000,
            json!([
                {"path": "a/1.txt", "size": 1, "source": "public"},
                {"path": "a/2.txt", "size": 1, "source": "public"},
            ]),
        );
        let description = synthesize(ANT_MANIFEST_MIME, None, &meta);
        assert_eq!(
            description,
            "an autonomi file manifest titled \"big\" with 100000 entries"
        );
        assert!(
            !description.contains("containing"),
            "the stored prefix must not be presented as the manifest"
        );
    }

    #[test]
    fn watchlist_base_uses_only_last_segment_stem() {
        let meta = ExtractedMeta::default();
        // Full URL path: host + directories must not leak into the base.
        let base = watchlist_base(
            WATCHLIST_MIME,
            Some("github.com/aautonomicc/Watch-It/raw/main/catalog/Public Domain.watch-list"),
            &meta,
        );
        assert_eq!(base, "a public domain media watch list");
        assert!(!base.contains("github"));
        assert!(!base.contains("aautonomicc"));
        assert!(!base.contains("raw main"));
    }

    #[test]
    fn watchlist_base_bare_filename_uses_stem() {
        let meta = ExtractedMeta::default();
        let base = watchlist_base(WATCHLIST_MIME, Some("My Library.watch-list"), &meta);
        assert_eq!(base, "a my library media watch list");
    }

    #[test]
    fn watchlist_base_title_fallback() {
        let meta = ExtractedMeta {
            title: Some("Sci-Fi Canon".into()),
            ..Default::default()
        };
        let base = watchlist_base(WATCHLIST_MIME, None, &meta);
        assert_eq!(base, "a media watch list titled \"Sci-Fi Canon\"");
    }
}
