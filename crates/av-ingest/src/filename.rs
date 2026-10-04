/// Tokenize a filename stem into cleaned human-readable terms.
/// Input: filename with or without extension (e.g. "cheesy_fish-01.jpg" or "cheesy_fish-01")
/// Output: "cheesy fish" (drops numeric-only "01", joins rest)
pub fn tokenize_filename(filename: &str) -> String {
    // Strip extension
    let stem = match filename.rfind('.') {
        Some(i) if i > 0 => &filename[..i],
        _ => filename,
    };

    // Split on separators
    let tokens: Vec<&str> = stem
        .split(|c: char| c == '_' || c == '-' || c == '.' || c == ' ' || c == '+')
        .collect();

    // Clean and filter
    let words: Vec<String> = tokens
        .iter()
        .map(|t| {
            t.trim()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|t| !t.is_empty() && t.len() > 1 && !t.chars().all(|c| c.is_ascii_digit()))
        .collect();

    words.join(" ")
}

/// Returns None if no useful tokens were found.
pub fn tokenize_filename_opt(filename: &str) -> Option<String> {
    let s = tokenize_filename(filename);
    if s.is_empty() { None } else { Some(s) }
}

/// Pick a download filename for content fetched from a location.
///
/// A bare `autonomi://<hex>` carries no name, and the SDK's `file.name` is not
/// one either: it is always a synthetic `public-file-<8hex>.bin` derived from the
/// record's public key. Handing that to a browser's download attribute would put
/// every file on a user's disk under the same meaningless name, so the name is
/// taken from the location instead:
///
/// 1. A trailing path segment, when the location has one (`autonomi://<hex>/report.pdf`)
///    — the human-facing name, sanitized.
/// 2. Otherwise a short prefix of the address plus an extension implied by the
///    detected MIME type, so the file is at least recognisable and typed.
///
/// Anything found is reduced to a safe single path segment: a name taken from a
/// location is attacker-influenced input that ends up in a browser's download
/// directory, so separators and `..` cannot be allowed through.
pub fn suggest_filename(location: &str, mime: &str) -> String {
    let sanitized = |raw: &str| -> String {
        let cleaned: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
            .take(80)
            .collect();
        // `..`, a leading dot and a bare extension separator are all ways to
        // write a name that means something other than what it looks like.
        cleaned.trim().trim_start_matches('.').replace("..", "")
    };

    let address = location
        .rsplit("://")
        .next()
        .unwrap_or(location)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    // A trailing segment is the author's chosen name, so prefer it over anything
    // derived from the address.
    let trailing = location
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .rsplit_once("://")
        .map(|(_, tail)| tail)
        .and_then(|tail| tail.split_once('/'))
        .map(|(_, name)| sanitized(name))
        .filter(|name| !name.is_empty());

    if let Some(name) = trailing {
        return name;
    }

    let stem = if address.len() >= 16 {
        address[..16].to_string()
    } else if address.is_empty() {
        "download".to_string()
    } else {
        address
    };
    let ext = extension_for_mime(mime);
    match ext {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem,
    }
}

/// Extension for the MIME types worth naming; `None` rather than a wrong guess.
fn extension_for_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next().unwrap_or(mime).trim();
    Some(match mime {
        "text/html" => "html",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "text/css" => "css",
        "application/json" => "json",
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/wav" => "wav",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "application/zip" => "zip",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::suggest_filename;

    const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_trailing_segment_is_the_authors_name() {
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}/report.pdf"), "application/pdf"),
            "report.pdf"
        );
    }

    #[test]
    fn a_bare_address_falls_back_to_a_short_prefix_and_a_typed_extension() {
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}"), "application/pdf"),
            "0123456789abcdef.pdf"
        );
    }

    #[test]
    fn an_unknown_type_gets_no_invented_extension() {
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}"), "application/x-unknown"),
            "0123456789abcdef"
        );
    }

    #[test]
    fn a_name_cannot_escape_the_download_directory() {
        for hostile in [
            "../../etc/passwd",
            "..\\..\\windows\\system32\\evil.exe",
            "/etc/passwd",
            "...",
        ] {
            let name = suggest_filename(&format!("autonomi://{HEX}/{hostile}"), "text/plain");
            assert!(!name.contains('/'), "{hostile} produced {name}");
            assert!(!name.contains('\\'), "{hostile} produced {name}");
            assert!(!name.contains(".."), "{hostile} produced {name}");
            assert!(name.len() < 90, "{hostile} produced {name}");
        }
    }

    #[test]
    fn a_location_with_a_query_still_finds_its_name() {
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}/notes.txt?x=1"), "text/plain"),
            "notes.txt"
        );
    }
}
