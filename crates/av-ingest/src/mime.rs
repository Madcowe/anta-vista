use crate::error::{IngestError, IngestResult};

/// Detect MIME type from the first bytes of file content.
/// Returns a MIME string like "image/jpeg", "audio/mpeg", "application/pdf", "text/plain", etc.
pub fn detect_mime(bytes: &[u8]) -> IngestResult<String> {
    if bytes.is_empty() {
        return Err(IngestError::FileTooSmall);
    }

    // Use the `infer` crate for content inspection
    if let Some(kind) = infer::get(bytes) {
        return Ok(kind.mime_type().to_string());
    }

    // Fallback: check if it looks like UTF-8 text
    if std::str::from_utf8(bytes).is_ok() {
        // Further check for HTML
        let trimmed = bytes.iter().take(512).copied().collect::<Vec<_>>();
        let s = String::from_utf8_lossy(&trimmed).to_lowercase();
        if s.contains("<!doctype html") || s.contains("<html") {
            return Ok("text/html".to_string());
        }
        return Ok("text/plain".to_string());
    }

    Ok("application/octet-stream".to_string())
}

/// Canonicalise a MIME string: lowercase, strip parameters.
pub fn canonicalize_mime(mime: &str) -> String {
    mime.split(';').next().unwrap_or(mime).trim().to_lowercase()
}

/// Extract major type ("image", "audio", "video", "text", "application")
pub fn mime_major(mime: &str) -> &str {
    mime.split('/').next().unwrap_or("application")
}

/// Extract subtype ("jpeg", "mpeg", "pdf", "plain", ...)
pub fn mime_sub(mime: &str) -> &str {
    mime.split('/').nth(1).unwrap_or("octet-stream")
}
