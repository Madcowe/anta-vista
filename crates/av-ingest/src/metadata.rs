use serde_json::{json, Value};

/// Extracted metadata from a resource.
#[derive(Debug, Default)]
pub struct ExtractedMeta {
    /// Human-readable title if found (from EXIF ImageDescription, ID3 title, PDF title, first line of text)
    pub title: Option<String>,
    /// Artist/author if found (from ID3 artist)
    pub artist: Option<String>,
    /// Album if found (from ID3 album)
    pub album: Option<String>,
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
        ("text", "plain") => extract_text(bytes),
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
        // First non-empty line as title
        let first_line = s
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .map(|l| l.chars().take(120).collect::<String>());
        meta.title = first_line;
    }
    meta
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
