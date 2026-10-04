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

/// Pick a download filename for content fetched from a location, using only what
/// the location itself says. See [`suggest_filename_with`] for the version that
/// can also be told a name we already hold.
pub fn suggest_filename(location: &str, mime: &str) -> String {
    suggest_filename_with(location, None, mime)
}

/// Pick a download filename, optionally preferring a name we already hold.
///
/// The precedence is deliberate and ordered by how much the author meant it:
///
/// 1. A trailing path segment or `?name=` on the location, when there is one
///    (`autonomi://<hex>/report.pdf`) — the author's live intent, spelled out in
///    the link the user is looking at right now.
/// 2. `known_name`, a name we stored when we indexed the record. This is what
///    rescues the bare-address case: an Autonomi record's *canonical* location
///    is the address alone, because that is its identity for dedup and blocking,
///    so a search result carries `autonomi://<hex>` and the `/welcome.md` the
///    author published is only in `resources.filename`. Without this the name is
///    written to one field and read from a different one, and every download is
///    named after the first 16 hex of the address.
/// 3. A short address prefix plus an extension implied by the detected MIME.
///
/// The later sources never override an earlier one, so a stored name cannot
/// mask a name the link still carries.
///
/// Anything found is reduced to a safe single path segment: a name taken from a
/// location is attacker-influenced input that ends up in a browser's download
pub fn suggest_filename_with(location: &str, known_name: Option<&str>, mime: &str) -> String {
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
    // derived from the address. Sourced from the location layer rather than
    // re-parsed here, so percent-decoding and the sanitising rules cannot drift
    // between the two; this function's stricter `sanitized` still runs over the
    // result, because this name ends up in a header and a download directory.
    let trailing = crate::location::last_url_segment(location)
        // `?name=` carries the author's name too, and for a bare Autonomi record
        // with no path it is the *only* place one can appear — which this used
        // to ignore, falling back to an address-derived name.
        .or_else(|| crate::location::analyze_location(location).inferred_filename)
        .or_else(|| known_name.map(str::to_string))
        .map(|name| sanitized(&name))
        .filter(|name| !name.is_empty());

    if let Some(name) = trailing {
        // The author chose this name, so it wins over anything derived. It still
        // gets an extension when it has none: we already know the type, and a
        // file saved as `photo` is one the operating system cannot open, which
        // is the same reason the bare-address path below types its fallback.
        // Never *replaces* an extension that is already there — an unrecognised
        // one is still the author's, and overriding it would let the content
        // rename the file they published.
        let stem = name.trim_end_matches('.');
        if crate::mime::trusted_extension(stem).is_some() {
            return name;
        }
        return match extension_for_mime(mime) {
            Some(ext) => format!("{stem}.{ext}"),
            // Nothing to add, but a stray trailing separator is still cleaned up.
            None => stem.to_string(),
        };
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
///
/// The inverse of `mime::HINTED_TYPES`, and kept complete against it: a type we
/// are willing to *resolve* on a name is a type we should be willing to *name*,
/// or the two disagree and a `.csv` file is downloaded as an untyped blob.
/// `every_type_we_would_resolve_can_also_be_named` pins the pairing.
pub(crate) fn extension_for_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next().unwrap_or(mime).trim();
    Some(match mime {
        "text/html" => "html",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "text/css" => "css",
        "text/csv" => "csv",
        "application/json" => "json",
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        "image/vnd.microsoft.icon" => "ico",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/wav" => "wav",
        "audio/flac" => "flac",
        "audio/aac" => "aac",
        "audio/mp4" => "m4a",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "application/zip" => "zip",
        "application/xhtml+xml" => "xhtml",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{suggest_filename, suggest_filename_with};

    #[test]
    fn a_known_name_is_used_when_the_location_carries_none() {
        // The bare-address case, and the reason this function takes a name at
        // all: the canonical location of an Autonomi record is the address
        // alone, so the `/welcome.md` the author published reaches us only as a
        // stored column.
        let uri = format!("autonomi://{HEX}");
        assert_eq!(
            suggest_filename(&uri, "text/markdown"),
            "0123456789abcdef.md"
        );
        assert_eq!(
            suggest_filename_with(&uri, Some("welcome.md"), "text/markdown"),
            "welcome.md"
        );
    }

    #[test]
    fn the_location_still_outranks_a_name_we_hold() {
        // Ordered by how much the author meant it: the link in front of the user
        // is live intent, and a stored name must not mask it.
        let uri = format!("autonomi://{HEX}/report.pdf");
        assert_eq!(
            suggest_filename_with(&uri, Some("welcome.md"), "application/pdf"),
            "report.pdf"
        );
        let query = format!("autonomi://{HEX}?name=notes");
        assert_eq!(
            suggest_filename_with(&query, Some("welcome.md"), "text/plain"),
            "notes.txt"
        );
    }

    #[test]
    fn a_known_name_gets_an_extension_and_keeps_its_own() {
        let uri = format!("autonomi://{HEX}");
        assert_eq!(
            suggest_filename_with(&uri, Some("notes"), "text/markdown"),
            "notes.md"
        );
        // An unrecognised extension is still the author's, and overriding it
        // would let the content rename the file they published.
        assert_eq!(
            suggest_filename_with(&uri, Some("notes.xyz"), "text/markdown"),
            "notes.xyz"
        );
    }

    #[test]
    fn an_empty_or_blank_known_name_is_no_name_at_all() {
        let uri = format!("autonomi://{HEX}");
        for blank in ["", "   ", "..", "."] {
            assert_eq!(
                suggest_filename_with(&uri, Some(blank), "text/markdown"),
                "0123456789abcdef.md",
                "{blank:?} must not produce an empty or dotted name"
            );
        }
    }

    #[test]
    fn a_bare_address_is_typed_from_what_we_know_about_the_content() {
        // A record published at its raw address has no name to read, so the stem
        // is the address and the extension has to come from the sniffed type.
        // Markdown reached here as an untyped blob: `infer` has no signature for
        // it, the SDK reported octet-stream, and neither the type nor the name
        // was available, so it arrived with no extension at all.
        let uri = "autonomi://52af8a181c6171f8e12896ca734cdf60839c3b324dfbeadf857226dfc7cb453b";
        assert_eq!(
            suggest_filename(uri, "text/markdown"),
            "52af8a181c6171f8.md"
        );
        assert_eq!(
            suggest_filename(uri, "application/octet-stream"),
            "52af8a181c6171f8",
            "with no usable type there is still nothing honest to add"
        );
    }

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
    fn a_nameless_extension_is_added_when_the_type_is_known() {
        // The author named it `photo`; we know it is a JPEG. Downloading it as
        // `photo` leaves the operating system unable to open it, which is the
        // whole reason the bare-address path appends one.
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}/photo"), "image/jpeg"),
            "photo.jpg"
        );
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}/report"), "application/pdf"),
            "report.pdf"
        );
        // `?name=` carries the author's name the same way a path does.
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}?name=notes"), "text/plain"),
            "notes.txt"
        );
    }

    #[test]
    fn an_extension_the_author_chose_is_never_replaced() {
        // Appending is for a name that has none. Overriding a name that already
        // carries one would let the *content* rename the file the author asked
        // for, and the two are independent: a `.pdf` that sniffs as HTML is
        // still the document they published.
        for (name, mime, expected) in [
            ("report.pdf", "application/pdf", "report.pdf"),
            // An extension we do not recognise is still an extension: it is the
            // author's, and rewriting it to our guess loses information.
            ("notes.xyz", "image/jpeg", "notes.xyz"),
            ("archive.tar.gz", "application/zip", "archive.tar.gz"),
        ] {
            assert_eq!(
                suggest_filename(&format!("autonomi://{HEX}/{name}"), mime),
                expected,
                "{name} with {mime}"
            );
        }
    }

    #[test]
    fn a_trailing_separator_does_not_produce_a_double_dot() {
        // `report.` has no extension, so one is added — without producing
        // `report..pdf`, which is what a naive append gives.
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}/report."), "application/pdf"),
            "report.pdf"
        );
        // And with nothing to append, the stray dot is still cleaned up.
        assert_eq!(
            suggest_filename(
                &format!("autonomi://{HEX}/report."),
                "application/x-unknown"
            ),
            "report"
        );
    }

    #[test]
    fn a_location_with_a_query_still_finds_its_name() {
        assert_eq!(
            suggest_filename(&format!("autonomi://{HEX}/notes.txt?x=1"), "text/plain"),
            "notes.txt"
        );
    }
}
