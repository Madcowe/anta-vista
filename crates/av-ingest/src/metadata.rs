use serde_json::{json, Value};

const MAX_PREVIEW_CHARS: usize = 2000;

/// Extracted metadata from a resource.
#[derive(Debug, Default)]
pub struct ExtractedMeta {
    /// Human-readable title if found (from EXIF ImageDescription, ID3 title, PDF title, first line of text)
    pub title: Option<String>,
    /// Artist/author if found (from ID3 artist)
    pub artist: Option<String>,
    /// Album if found (from ID3 album)
    pub album: Option<String>,
    /// Clean text preview (first ~1000 chars) for text-based documents
    pub content_preview: Option<String>,
    /// Arbitrary additional key-value metadata
    pub extra: Value,
}

/// Extract metadata from content bytes, using the detected MIME type as a hint.
pub fn extract(bytes: &[u8], mime: &str) -> ExtractedMeta {
    let major = mime.split('/').next().unwrap_or("");
    let sub = mime.split('/').nth(1).unwrap_or("");

    match (major, sub) {
        ("image", _) => extract_exif(bytes),
        ("audio", _) => extract_id3(bytes),
        ("text", "html") => extract_html(bytes),
        ("text", "markdown") => extract_text(bytes),
        ("text", "plain") => extract_text(bytes),
        ("text", _) => extract_text(bytes),
        ("application", "pdf") => extract_pdf(bytes),
        _ => ExtractedMeta::default(),
    }
}

fn extract_exif(bytes: &[u8]) -> ExtractedMeta {
    use std::io::Cursor;
    let mut meta = ExtractedMeta::default();
    let cursor = Cursor::new(bytes);
    if let Ok(exif_reader) =
        exif::Reader::new().read_from_container(&mut std::io::BufReader::new(cursor))
    {
        // Try to get ImageDescription
        if let Some(field) = exif_reader.get_field(exif::Tag::ImageDescription, exif::In::PRIMARY) {
            let val = field.display_value().to_string();
            let trimmed = val.trim_matches('"').trim().to_string();
            meta.title = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
        // Extra: DateTime
        if let Some(field) = exif_reader.get_field(exif::Tag::DateTime, exif::In::PRIMARY) {
            meta.extra = json!({ "datetime": field.display_value().to_string() });
        }
    }
    meta
}

fn extract_id3(bytes: &[u8]) -> ExtractedMeta {
    use id3::TagLike;
    use std::io::Cursor;
    let mut meta = ExtractedMeta::default();
    if let Ok(tag) = id3::Tag::read_from2(Cursor::new(bytes)) {
        meta.title = tag.title().map(|s| s.to_string());
        meta.artist = tag.artist().map(|s| s.to_string());
        meta.album = tag.album().map(|s| s.to_string());
        meta.extra = json!({
            "title":  meta.title,
            "artist": meta.artist,
            "album":  meta.album,
        });
    }
    meta
}

fn extract_text(bytes: &[u8]) -> ExtractedMeta {
    let mut meta = ExtractedMeta::default();
    if let Ok(s) = std::str::from_utf8(bytes) {
        let first_line = s
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .map(|l| l.chars().take(120).collect::<String>());
        meta.title = first_line;
        meta.content_preview = Some(s.chars().take(MAX_PREVIEW_CHARS).collect());
    }
    meta
}

fn extract_html(bytes: &[u8]) -> ExtractedMeta {
    let mut meta = ExtractedMeta::default();
    if let Ok(s) = std::str::from_utf8(bytes) {
        if let Some(start) = s.find("<title>") {
            let after = &s[start + 7..];
            if let Some(end) = after.find("</title>") {
                let title = after[..end].trim().to_string();
                if !title.is_empty() {
                    meta.title = Some(title);
                }
            }
        }
        // Use <meta name="description"> (or og:twitter:description) as the
        // content preview — it's a human-written summary, zero boilerplate.
        meta.content_preview = extract_meta_description(s);
    }
    meta
}

/// Extract content from the first <meta name="description">,
/// <meta property="og:description">, or <meta name="twitter:description"> tag.
fn extract_meta_description(html: &str) -> Option<String> {
    let mut search = 0;
    while let Some(start) = html[search..].find("<meta") {
        let abs = search + start;
        let rest = &html[abs..];
        let close = rest.find('>')?;
        let tag = &rest[..close + 1];
        search = abs + 1;

        let lower = tag.to_ascii_lowercase();
        let is_description = lower.contains("name=\"description\"")
            || lower.contains("name='description'")
            || lower.contains("property=\"og:description\"")
            || lower.contains("property='og:description'")
            || lower.contains("name=\"twitter:description\"")
            || lower.contains("name='twitter:description'");

        if is_description {
            if let Some(val) = extract_attr_value(tag, "content") {
                let trimmed = val.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }
    None
}

/// Extract an HTML attribute value (double-quoted or single-quoted).
fn extract_attr_value(tag: &str, attr: &str) -> Option<String> {
    let dq = format!("{}=\"", attr);
    if let Some(pos) = tag.find(&dq) {
        let val_start = pos + dq.len();
        if let Some(end) = tag[val_start..].find('"') {
            return Some(tag[val_start..val_start + end].to_string());
        }
    }
    let sq = format!("{}='", attr);
    if let Some(pos) = tag.find(&sq) {
        let val_start = pos + sq.len();
        if let Some(end) = tag[val_start..].find('\'') {
            return Some(tag[val_start..val_start + end].to_string());
        }
    }
    None
}

fn extract_pdf(bytes: &[u8]) -> ExtractedMeta {
    let mut meta = ExtractedMeta::default();
    // Minimal: scan first 2 KB for PDF /Title entry
    let window = &bytes[..bytes.len().min(2048)];
    if let Ok(s) = std::str::from_utf8(window) {
        if let Some(pos) = s.find("/Title") {
            let after = &s[pos + 6..];
            // Simple extraction of (title text) pattern
            if let Some(start) = after.find('(') {
                if let Some(end) = after[start + 1..].find(')') {
                    let title = after[start + 1..start + 1 + end].trim().to_string();
                    if !title.is_empty() {
                        meta.title = Some(title);
                    }
                }
            }
        }
    }
    meta
}
