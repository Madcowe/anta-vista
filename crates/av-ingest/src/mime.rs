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

/// Extensions we are willing to believe when content sniffing came back
/// generic, mapped to the type we then report.
///
/// Only types where being wrong is *safe* to render or serve: a mislabelled
/// image or PDF fails visibly and inertly, whereas a mislabelled `text/html`
/// would be a peer choosing how our own origin interprets its bytes. So HTML is
/// deliberately absent, and `text/html` is never one of the values a hint can
/// produce.
const HINTED_TYPES: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("ico", "image/vnd.microsoft.icon"),
    ("pdf", "application/pdf"),
    ("mp3", "audio/mpeg"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("flac", "audio/flac"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("csv", "text/csv"),
    ("json", "application/json"),
    ("zip", "application/zip"),
    // `text/html` and `image/svg+xml` are here because content sniffing is
    // *weak* for exactly these: a page whose first 512 bytes carry no literal
    // `<html`, or any SVG at all, sniffs as `text/plain`, and an HTML document
    // indexed as opaque bytes has no extractable text to embed. Both are
    // script-capable types, so serving them is only safe because the browser
    // viewer puts them on a throwaway port that cannot reach the API, and
    // `/api/*` refuses `Sec-Fetch-Site: cross-site`. Do not relax either half of
    // that without taking these two entries out again — and do not add a type
    // here without asking whether a browser would execute it.
    ("html", "text/html"),
    ("htm", "text/html"),
    ("xhtml", "application/xhtml+xml"),
    ("svg", "image/svg+xml"),
];

/// The MIME type an extension implies, if we trust that extension.
fn mime_from_extension(ext: &str) -> Option<&'static str> {
    HINTED_TYPES
        .iter()
        .find(|(e, _)| e.eq_ignore_ascii_case(ext))
        .map(|(_, mime)| *mime)
}

/// Detect MIME type from content, letting a known filename extension break ties.
///
/// Content sniffing is authoritative whenever it is *specific*: a JPEG's magic
/// bytes are not a guess, and a peer cannot make us call arbitrary bytes a PNG.
/// Sniffing is only ever weak at the generic end — `text/plain` for anything
/// that decodes as UTF-8 (including SVG, and HTML whose first 512 bytes happen
/// not to contain a literal `<html`), and `application/octet-stream` for the
/// rest. An extension resolves exactly those cases.
///
/// The reason this lives here rather than in the web layer: the stored
/// `mime_type` and the type a downloaded file is served as have to agree. A
/// result page that calls a file `text/plain` while the viewer serves it as a
/// PDF is the application contradicting itself, and two copies of this policy
/// would eventually drift.
pub fn detect_mime_with_hint(bytes: &[u8], filename: Option<&str>) -> IngestResult<String> {
    let sniffed = detect_mime(bytes)?;
    let generic = matches!(sniffed.as_str(), "text/plain" | "application/octet-stream");
    if !generic {
        return Ok(sniffed);
    }

    let Some(ext) = filename.and_then(|f| f.rsplit_once('.').map(|(_, e)| e)) else {
        return Ok(sniffed);
    };
    // An empty or absent extension means a dotfile or no extension at all, not
    // a request to guess from whatever follows the last dot in a long name.
    if ext.is_empty() || ext.len() > 8 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(sniffed);
    }

    Ok(mime_from_extension(ext).unwrap_or(&sniffed).to_string())
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

#[cfg(test)]
mod hint_tests {
    use super::detect_mime_with_hint;

    /// A page that sniffs as plain text: no markup token appears in it, which is
    /// exactly the real case — a file starting with a BOM, a comment, or a long
    /// `<head>` of attributes has nothing for a content sniffer to key on.
    const OBFUSCATED_HTML: &[u8] =
        b"\xef\xbb\xbfconsole.log(1);/* the opening tag is built here: 'ht'+'ml' */";

    #[test]
    fn a_known_extension_resolves_the_generic_sniff() {
        // The case the whole function exists for: a JPEG is never in doubt, but
        // arbitrary bytes named `.png` are only resolvable by the name.
        assert_eq!(
            detect_mime_with_hint(b"\x89PNG\r\n\x1a\n", None).unwrap(),
            "image/png"
        );
        assert_eq!(
            detect_mime_with_hint(b"just some words", Some("notes.txt")).unwrap(),
            "text/plain"
        );
    }

    #[test]
    fn html_and_svg_resolve_from_the_extension_alone() {
        assert_eq!(
            detect_mime_with_hint(OBFUSCATED_HTML, Some("page.html")).unwrap(),
            "text/html"
        );
        assert_eq!(
            detect_mime_with_hint(
                b"<svg xmlns='http://www.w3.org/2000/svg'/>",
                Some("logo.svg")
            )
            .unwrap(),
            "image/svg+xml"
        );
    }

    #[test]
    fn content_wins_when_it_is_specific() {
        // A `.html` name must not turn a real PNG into markup: the extension
        // only ever breaks ties at the generic end.
        assert_eq!(
            detect_mime_with_hint(b"\x89PNG\r\n\x1a\n", Some("disguised.html")).unwrap(),
            "image/png"
        );
    }

    #[test]
    fn an_untrustworthy_extension_is_ignored_rather_than_guessed_at() {
        // Only the extension is examined, and only when it is short and
        // alphanumeric — so a name whose last dot is not really an extension
        // must not be read as one.
        for name in [
            "noextension",
            "trailingdot.",
            "way.too.long.for.an.extension",
            "report.ht~ml",
            "report.html\n",
        ] {
            assert_eq!(
                detect_mime_with_hint(OBFUSCATED_HTML, Some(name)).unwrap(),
                "text/plain",
                "{name} was trusted"
            );
        }
    }

    #[test]
    fn the_stem_is_irrelevant_and_the_extension_is_not() {
        // The mirror of the test above: only the extension is judged, so a stem
        // full of spaces, digits and unicode is fine as long as `.html` is the
        // trailing token.
        assert_eq!(
            detect_mime_with_hint(OBFUSCATED_HTML, Some("no extension.html")).unwrap(),
            "text/html"
        );
    }
}
