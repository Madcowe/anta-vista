#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationInfo {
    pub scheme: Option<String>,
    pub canonical: Option<String>,
    pub inferred_filename: Option<String>,
}

/// If the input has no `://` scheme but starts with 64 hex characters
/// (with optional `/path`, `?query`, or `#fragment`), treat it as an `autonomi://` URI.
pub fn normalize_uri(uri: &str) -> String {
    if uri.contains("://") {
        return uri.to_string();
    }
    let (head, _) = split_once_any(uri, &['/', '?', '#']);
    if is_64_hex(head) {
        return format!("autonomi://{}", uri);
    }
    uri.to_string()
}

pub fn analyze_location(location: &str) -> LocationInfo {
    let location = normalize_uri(location);
    let Some((raw_scheme, rest)) = location.split_once("://") else {
        return LocationInfo {
            scheme: None,
            canonical: None,
            inferred_filename: None,
        };
    };

    let scheme = av_core::types::normalize_scheme(raw_scheme);

    if scheme != "autonomi" {
        let inferred_filename = extract_url_context(rest);
        return LocationInfo {
            scheme: Some(scheme),
            canonical: Some(location.to_string()),
            inferred_filename,
        };
    }

    match parse_ant_location(rest) {
        Some(parsed) => LocationInfo {
            scheme: Some(scheme),
            canonical: Some(format!("autonomi://{}", parsed.address)),
            inferred_filename: parsed.filename,
        },
        None => LocationInfo {
            scheme: Some(scheme),
            canonical: Some(location.to_string()),
            inferred_filename: None,
        },
    }
}

struct ParsedAntLocation {
    address: String,
    filename: Option<String>,
}

fn parse_ant_location(rest: &str) -> Option<ParsedAntLocation> {
    let (without_fragment, _) = split_once_any(rest, &['#']);
    let (before_query, query) = split_once_any(without_fragment, &['?']);
    let (authority, path) = split_path(before_query);

    if !is_64_hex(authority) {
        return None;
    }

    let filename = filename_from_path(path).or_else(|| query.and_then(filename_from_query));

    Some(ParsedAntLocation {
        address: authority.to_ascii_lowercase(),
        filename,
    })
}

fn split_once_any<'a>(value: &'a str, delimiters: &[char]) -> (&'a str, Option<&'a str>) {
    match value.find(delimiters) {
        Some(index) => (&value[..index], Some(&value[index + 1..])),
        None => (value, None),
    }
}

fn split_path(value: &str) -> (&str, Option<&str>) {
    match value.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (value, None),
    }
}

fn filename_from_path(path: Option<&str>) -> Option<String> {
    let path = path?;
    let segment = path.rsplit('/').find(|segment| !segment.is_empty())?;
    sanitize_filename(&percent_decode(segment)?)
}

fn filename_from_query(query: &str) -> Option<String> {
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "name" {
            return sanitize_filename(&percent_decode(value)?);
        }
    }
    None
}

fn sanitize_filename(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_value(bytes[i + 1])?;
            let lo = hex_value(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn extract_url_context(rest: &str) -> Option<String> {
    let (before_fragment, _) = split_once_any(rest, &['#']);
    let (before_query, _) = split_once_any(before_fragment, &['?']);
    let (authority, path) = split_path(before_query);

    let authority_tokens: Vec<&str> = authority
        .split('.')
        .filter(|s| !s.is_empty() && *s != "www")
        .collect();

    let segments: Vec<&str> = path
        .map(|p| p.split('/').filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    let (context_segments, filename_segment) = if let Some((last, rest)) = segments.split_last() {
        if last.contains('.') {
            (rest.to_vec(), Some(*last))
        } else {
            (segments.clone(), None)
        }
    } else {
        (vec![], None)
    };

    let meaningful: Vec<&str> = context_segments
        .iter()
        .filter(|s| is_meaningful_segment(s))
        .copied()
        .collect();

    let mut parts: Vec<String> = Vec::new();

    for t in &authority_tokens {
        if let Some(decoded) = percent_decode(t) {
            if is_meaningful_segment(&decoded) {
                parts.push(decoded);
            }
        }
    }

    for s in &meaningful {
        if let Some(decoded) = percent_decode(s) {
            parts.push(decoded);
        }
    }

    if let Some(fn_seg) = filename_segment {
        if let Some(decoded) = percent_decode(fn_seg) {
            if sanitize_filename(&decoded).is_some() {
                parts.push(decoded);
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn is_meaningful_segment(segment: &str) -> bool {
    if segment.len() <= 1 || segment == "." || segment == ".." {
        return false;
    }
    if segment.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if segment.len() > 16 && segment.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    if segment.len() == 36
        && segment.chars().filter(|c| *c == '-').count() == 4
        && segment.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return false;
    }
    true
}

fn is_64_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: &str = "711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a";

    #[test]
    fn infers_ant_path_filename() {
        let info = analyze_location(&format!("ant://{ADDRESS}/lucky.jpg"));
        assert_eq!(info.scheme.as_deref(), Some("autonomi"));
        assert_eq!(
            info.canonical.as_deref(),
            Some(format!("autonomi://{ADDRESS}").as_str())
        );
        assert_eq!(info.inferred_filename.as_deref(), Some("lucky.jpg"));
    }

    #[test]
    fn infers_ant_query_name() {
        let info = analyze_location(&format!("ant://{ADDRESS}?name=lucky.jpg"));
        assert_eq!(
            info.canonical.as_deref(),
            Some(format!("autonomi://{ADDRESS}").as_str())
        );
        assert_eq!(info.inferred_filename.as_deref(), Some("lucky.jpg"));
    }

    #[test]
    fn normalizes_autonomi_alias() {
        let info = analyze_location(&format!("autonomi://{ADDRESS}?name=lucky.jpg"));
        assert_eq!(info.scheme.as_deref(), Some("autonomi"));
        assert_eq!(
            info.canonical.as_deref(),
            Some(format!("autonomi://{ADDRESS}").as_str())
        );
        assert_eq!(info.inferred_filename.as_deref(), Some("lucky.jpg"));
    }

    #[test]
    fn path_filename_wins_over_query_name() {
        let info = analyze_location(&format!("ant://{ADDRESS}/path.jpg?name=query.jpg"));
        assert_eq!(info.inferred_filename.as_deref(), Some("path.jpg"));
    }

    #[test]
    fn decodes_filename_hints() {
        let info = analyze_location(&format!("ant://{ADDRESS}?name=lucky%20cat.jpg"));
        assert_eq!(info.inferred_filename.as_deref(), Some("lucky cat.jpg"));
    }

    #[test]
    fn ignores_unsafe_query_name() {
        let info = analyze_location(&format!("ant://{ADDRESS}?name=../secret.jpg"));
        assert_eq!(info.inferred_filename, None);
    }

    #[test]
    fn extracts_path_filename_from_http() {
        let info = analyze_location("https://example.com/images/photo.jpg");
        assert_eq!(info.scheme.as_deref(), Some("https"));
        assert_eq!(
            info.canonical.as_deref(),
            Some("https://example.com/images/photo.jpg")
        );
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("example com images photo.jpg")
        );
    }

    #[test]
    fn filters_numeric_path_segments() {
        let info = analyze_location("https://example.com/12345/report.pdf");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("example com report.pdf")
        );
    }

    #[test]
    fn filters_hex_path_segments() {
        let info =
            analyze_location("https://cdn.example.com/aB3dEfGhIjKlMnOpQrStUvWxYz123456/photo.jpg");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("cdn example com photo.jpg")
        );
    }

    #[test]
    fn filters_single_char_segments() {
        let info = analyze_location("https://example.com/a/b/c/file.txt");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("example com file.txt")
        );
    }

    #[test]
    fn strips_www_prefix() {
        let info = analyze_location("https://www.example.com/images/photo.jpg");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("example com images photo.jpg")
        );
    }

    #[test]
    fn handles_wiki_style_path() {
        let info = analyze_location("https://en.wikipedia.org/wiki/Rust_programming_language");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("en wikipedia org wiki rust programming language")
        );
    }

    #[test]
    fn handles_percent_encoded_path() {
        let info = analyze_location("https://example.com/path%20with%20spaces/file.txt");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("example com path with spaces file.txt")
        );
    }

    #[test]
    fn handles_localhost_with_port() {
        let info = analyze_location("http://localhost:8080/images/photo.jpg");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("localhost images photo.jpg")
        );
    }

    #[test]
    fn handles_root_path_without_filename() {
        let info = analyze_location("https://example.com/");
        assert_eq!(info.inferred_filename, None);
    }

    #[test]
    fn handles_api_endpoint_without_extension() {
        let info = analyze_location("https://api.example.com/v2/products");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("api example com v2 products")
        );
    }

    #[test]
    fn ant_location_still_extracts_filename() {
        let info = analyze_location(&format!("ant://{ADDRESS}/lucky.jpg"));
        assert_eq!(info.inferred_filename.as_deref(), Some("lucky.jpg"));
        assert_eq!(
            info.canonical.as_deref(),
            Some(format!("autonomi://{ADDRESS}").as_str())
        );
    }

    #[test]
    fn filters_uuid_path_segments() {
        let info = analyze_location(
            "https://example.com/550e8400-e29b-41d4-a716-446655440000/report.pdf",
        );
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("example com report.pdf")
        );
    }

    #[test]
    fn preserves_deep_nested_paths() {
        let info = analyze_location("https://archive.org/details/some-book-title/chapter-5.pdf");
        assert_eq!(
            info.inferred_filename.as_deref(),
            Some("archive org details some book title chapter 5.pdf")
        );
    }
}
