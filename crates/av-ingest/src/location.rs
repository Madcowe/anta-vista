#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationInfo {
    pub scheme: Option<String>,
    pub canonical: Option<String>,
    pub inferred_filename: Option<String>,
}

pub fn analyze_location(location: &str) -> LocationInfo {
    let Some((raw_scheme, rest)) = location.split_once("://") else {
        return LocationInfo {
            scheme: None,
            canonical: None,
            inferred_filename: None,
        };
    };

    let scheme = av_core::types::normalize_scheme(raw_scheme);

    if scheme != "ant" {
        return LocationInfo {
            scheme: Some(scheme),
            canonical: Some(location.to_string()),
            inferred_filename: None,
        };
    }

    match parse_ant_location(rest) {
        Some(parsed) => LocationInfo {
            scheme: Some(scheme),
            canonical: Some(format!("ant://{}", parsed.address)),
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
        assert_eq!(info.scheme.as_deref(), Some("ant"));
        assert_eq!(
            info.canonical.as_deref(),
            Some(format!("ant://{ADDRESS}").as_str())
        );
        assert_eq!(info.inferred_filename.as_deref(), Some("lucky.jpg"));
    }

    #[test]
    fn infers_ant_query_name() {
        let info = analyze_location(&format!("ant://{ADDRESS}?name=lucky.jpg"));
        assert_eq!(
            info.canonical.as_deref(),
            Some(format!("ant://{ADDRESS}").as_str())
        );
        assert_eq!(info.inferred_filename.as_deref(), Some("lucky.jpg"));
    }

    #[test]
    fn normalizes_autonomi_alias() {
        let info = analyze_location(&format!("autonomi://{ADDRESS}?name=lucky.jpg"));
        assert_eq!(info.scheme.as_deref(), Some("ant"));
        assert_eq!(
            info.canonical.as_deref(),
            Some(format!("ant://{ADDRESS}").as_str())
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
    fn leaves_non_ant_locations_without_filename() {
        let info = analyze_location("https://example.com/lucky.jpg");
        assert_eq!(info.scheme.as_deref(), Some("https"));
        assert_eq!(
            info.canonical.as_deref(),
            Some("https://example.com/lucky.jpg")
        );
        assert_eq!(info.inferred_filename, None);
    }
}
