use crate::{filename::tokenize_filename_opt, metadata::ExtractedMeta};
use av_core::constants::{EMBED_WINDOW_CHARS, WATCHLIST_MIME};

/// Semantic label for a MIME type, used in description synthesis.
/// Returns (with_filename_label, no_filename_fallback).
pub fn semantic_label(mime: &str) -> (&'static str, &'static str) {
    match mime {
        "application/x-watch-list" => ("watch list", "media watch list"),
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
        use crate::watchlist::{items_from_meta, base_from_meta};
        let base = base_from_meta(meta)
            .unwrap_or_else(|| watchlist_base(mime, filename, meta));
        let items = crate::series::compact_items(&items_from_meta(meta));
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
