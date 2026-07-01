use crate::{filename::tokenize_filename_opt, metadata::ExtractedMeta};

/// Semantic label for a MIME type, used in description synthesis.
/// Returns (with_filename_label, no_filename_fallback).
pub fn semantic_label(mime: &str) -> (&'static str, &'static str) {
    match mime {
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
