use crate::error::{IngestError, IngestResult};

/// How much of the file the bytes we're sniffing actually are.
///
/// The distinction matters because "is this text?" is answered by looking at
/// *all* the bytes handed in. Give it a prefix of a binary file and the answer
/// is about the prefix, not the file: a prefix can be valid UTF-8 while the
/// file it came from is not, so the same call has to be more careful here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confidence {
    /// The whole file.
    Full,
    /// The leading bytes of a larger file.
    Prefix,
}

/// Detect MIME type from the first bytes of file content.
/// Returns a MIME string like "image/jpeg", "audio/mpeg", "application/pdf", "text/plain", etc.
pub fn detect_mime(bytes: &[u8]) -> IngestResult<String> {
    detect_mime_inner(bytes, Confidence::Full)
}

/// Sniff a leading slice of a file, with the same policy as a whole-file sniff.
///
/// Weaker than `detect_mime` in exactly one way, and the rule is worth stating
/// plainly: **a prefix may confirm a specific type, never assert a generic
/// one.** Magic bytes need only the first few bytes, and the HTML marker is a
/// positive signal, so both hold on a prefix. But "this whole file is text" is
/// inferred from the *absence* of a binary structure, and a prefix cannot
/// establish an absence: the invalid UTF-8 a binary contains usually sits past
/// the first few kilobytes. So a prefix that decodes cleanly answers
/// `application/octet-stream`, not `text/plain`.
///
/// Guessing `text/plain` here would be worse than admitting ignorance: it marks
/// the record displayable and then shows mojibake, whereas the honest answer
/// sends it to a download, which always works.
///
/// This exists because the Autonomi SDK reports `application/octet-stream` for
/// most real records - a DataMap chunk carries no content type - so without it a
/// JPEG arrives as octet-stream and is downloaded instead of shown.
pub fn detect_mime_prefix(bytes: &[u8], filename: Option<&str>) -> IngestResult<String> {
    // An empty record has no bytes to sniff, which is not an error here: the
    // caller is asking what to do with a possibly-empty file, and the honest
    // answer is "generic", not a failure.
    if bytes.is_empty() {
        return Ok(hint_from("application/octet-stream", filename));
    }
    Ok(hint_from(
        &detect_mime_inner(bytes, Confidence::Prefix)?,
        filename,
    ))
}

/// The one copy of "sniff the bytes", so the full-file and prefix paths cannot
/// drift into disagreeing about what a type is.
fn detect_mime_inner(bytes: &[u8], confidence: Confidence) -> IngestResult<String> {
    if bytes.is_empty() {
        return Err(IngestError::FileTooSmall);
    }

    // Use the `infer` crate for content inspection
    if let Some(kind) = infer::get(bytes) {
        return Ok(kind.mime_type().to_string());
    }

    // Autonomi file manifest (`.ant`): a magic like any other, so it holds on
    // a prefix too, and — unlike the HTML marker — it is a fixed header rather
    // than a token searched for. Keyed on the bytes alone, never on an `.ant`
    // name: a non-manifest file merely *called* `.ant` keeps its real sniff,
    // and only content ever asserts this type.
    if bytes.starts_with(crate::ant_manifest::MAGIC) {
        return Ok(av_core::constants::ANT_MANIFEST_MIME.to_string());
    }

    // Fallback: check if it looks like UTF-8 text
    if std::str::from_utf8(bytes).is_ok() {
        // Further check for HTML. A positive marker in the first 512 bytes, so
        // it survives truncation - this is what keeps a fetched web page
        // displayable when the SDK labels it octet-stream.
        let head = bytes.iter().take(512).copied().collect::<Vec<_>>();
        let s = String::from_utf8_lossy(&head).to_lowercase();
        if s.contains("<!doctype html") || s.contains("<html") {
            return Ok("text/html".to_string());
        }
        // Same reasoning, weaker marker: markdown has no signature `infer` can
        // read, so it is recognised by its conventions instead.
        if looks_like_markdown(&head) {
            return Ok("text/markdown".to_string());
        }
        return Ok(match confidence {
            Confidence::Full => "text/plain".to_string(),
            Confidence::Prefix => "application/octet-stream".to_string(),
        });
    }

    Ok("application/octet-stream".to_string())
}

/// Whether the head of a file follows a markdown convention.
///
/// Unlike the HTML check this reads structure at the *start* of the file rather
/// than searching for a token, because `text/markdown` has no magic number at
/// all: `infer` cannot see it, and neither can any other content sniffer. So
/// this is a convention, not a signature, and the honest thing is to say so.
///
/// The false-positive risk is what makes it acceptable here: both candidates are
/// text, so a wrong answer is `text/markdown` for a file that is really
/// `text/plain`, and the only visible consequence is whether the saved file is
/// called `.md` or `.txt`. That is not the trade the HTML branch makes — an HTML
/// misdetection hands a peer a say over how our origin is interpreted, which is
/// why the marker there is a literal `<html`.
fn looks_like_markdown(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let body = text.trim_start_matches('\u{feff}').trim_start();
    let first = body.lines().next().unwrap_or("").trim_end();

    // YAML/TOML front matter, which is otherwise invisible to a content sniffer.
    if first == "---" {
        return true;
    }
    // An ATX heading opening the document.
    first.starts_with("# ") || first.starts_with("#\t")
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
    Ok(hint_from(&detect_mime(bytes)?, filename))
}

/// The trailing extension of `name`, if it is one we are willing to treat as an extension.
///
/// Only the extension is examined, and only when it is short and alphanumeric,
/// so a name whose last dot is not really an extension must not be read as one.
/// Shared with `suggest_filename` so that "does this name carry an extension?"
/// has exactly one answer: a name we would not break a type tie on is also a
/// name we will not append an extension to.
pub fn trusted_extension(name: &str) -> Option<&str> {
    let ext = name.rsplit_once('.').map(|(_, e)| e)?;
    if ext.is_empty() || ext.len() > 8 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ext)
}

/// Resolve a generic sniff with a known extension, or leave it alone.
///
/// Shared by the whole-file and prefix sniffs so the extension policy has one
/// home. Only breaks ties at the generic end; see `HINTED_TYPES` for why.
fn hint_from(sniffed: &str, filename: Option<&str>) -> String {
    let generic = matches!(sniffed, "text/plain" | "application/octet-stream");
    if !generic {
        return sniffed.to_string();
    }

    let Some(ext) = filename.and_then(trusted_extension) else {
        return sniffed.to_string();
    };

    mime_from_extension(ext).unwrap_or(sniffed).to_string()
}

/// Whether a browser can usefully *display* this type, as opposed to merely
/// storing it.
///
/// This is the only copy of that policy. Two callers depend on it: the viewer
/// picks `inline` versus `attachment` from it, and `/api/view/plan` tells the
/// page which one to expect. They used to have to agree by hand — the viewer
/// hardcoded `inline` for everything, which for an unrenderable type produces a
/// blank tab rather than a file, and no amount of client-side cleverness fixes
/// a response the server has already committed to.
///
/// The bias is towards "no". A type we cannot confidently display is offered to
/// the user as a download, which always works; rendering one we cannot display
/// gives them an empty window and no explanation.
pub fn is_browser_renderable(mime: &str) -> bool {
    let mime = canonicalize_mime(mime);
    match (mime_major(&mime), mime_sub(&mime)) {
        // Rendered as text. `text/html` is here because opening a fetched
        // webpage *as a webpage* is the entire point, and it is safe only
        // because the viewer serves it from its own origin.
        ("text", "plain" | "html" | "css") => true,
        ("application", "json" | "pdf") => true,
        // `image/svg+xml` is script-capable like HTML, and gets the same
        // treatment: isolated origin, no opener, cross-site POST refused.
        ("image" | "video" | "audio", _) => true,
        // `application/octet-stream` is the "we could not tell" answer, so it is
        // explicitly not renderable, and so is anything unrecognised.
        _ => false,
    }
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
    use super::{detect_mime, detect_mime_prefix, detect_mime_with_hint};

    /// A page that sniffs as plain text: no markup token appears in it, which is
    /// exactly the real case — a file starting with a BOM, a comment, or a long
    /// `<head>` of attributes has nothing for a content sniffer to key on.
    const OBFUSCATED_HTML: &[u8] =
        b"\xef\xbb\xbfconsole.log(1);/* the opening tag is built here: 'ht'+'ml' */";

    #[test]
    fn only_manifest_content_asserts_a_manifest_type() {
        // The `.ant` name resolves nothing: the type is a magic, so the
        // extension stays off the hint list, and a text file merely called
        // `.ant` keeps its real sniff. The magic — all four bytes of it, so it
        // survives a prefix sniff — asserts it regardless of what the name
        // claims the other way.
        assert_eq!(
            detect_mime_with_hint(b"just some words", Some("site.ant")).unwrap(),
            "text/plain"
        );
        assert_eq!(
            detect_mime(crate::ant_manifest::MAGIC).unwrap(),
            av_core::constants::ANT_MANIFEST_MIME
        );
        assert_eq!(
            detect_mime_prefix(crate::ant_manifest::MAGIC, Some("page.html")).unwrap(),
            av_core::constants::ANT_MANIFEST_MIME,
            "content wins over a name that claims something else"
        );
    }

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

#[cfg(test)]
mod prefix_tests {
    use super::{detect_mime_prefix, detect_mime_with_hint, is_browser_renderable};

    /// A file that is *not* valid UTF-8, but whose leading bytes decode cleanly
    /// and contain no NUL - which is why only a prefix sniff can be fooled by
    /// it. The invalid bytes sit past the prefix, as they do in a real binary.
    const CLEAN_PREFIX: &[u8] = b"header bytes that decode cleanly, no NUL, no BOM";
    const INVALID_UTF8_PAST_IT: &[u8] = &[0x9d, 0x00, 0x8f];

    fn binary_behind_a_clean_prefix() -> Vec<u8> {
        let mut bytes = CLEAN_PREFIX.to_vec();
        bytes.extend_from_slice(INVALID_UTF8_PAST_IT);
        bytes
    }

    #[test]
    fn a_magic_number_survives_being_truncated() {
        // The real fix: the SDK hands us `application/octet-stream` for a real
        // JPEG, and its magic bytes are in the first three bytes.
        for (magic, expected) in [
            (&b"\xff\xd8\xff\xe0"[..], "image/jpeg"),
            (&b"\x89PNG\r\n\x1a\n"[..], "image/png"),
            (&b"%PDF-1.7\n"[..], "application/pdf"),
            (&b"GIF89a"[..], "image/gif"),
        ] {
            assert_eq!(detect_mime_prefix(magic, None).unwrap(), expected);
            assert!(
                is_browser_renderable(&detect_mime_prefix(magic, None).unwrap()),
                "{expected} should be displayable"
            );
        }
    }

    #[test]
    fn html_is_still_recognised_from_a_prefix() {
        let page = b"<!DOCTYPE html>\n<html><body><p>hello, a page with a lot of text in it</p></body></html>";
        assert_eq!(detect_mime_prefix(page, None).unwrap(), "text/html");
        assert_eq!(detect_mime_prefix(&page[..64], None).unwrap(), "text/html");
    }

    #[test]
    fn a_prefix_that_decodes_cleanly_is_not_called_text() {
        // The reason this is a separate function rather than a shared code path
        // with `detect_mime`. These same bytes are text when they are the whole
        // file and generic when they are a prefix, and guessing otherwise marks
        // the record displayable and then shows mojibake.
        let bytes = binary_behind_a_clean_prefix();
        let prefix = CLEAN_PREFIX;
        assert!(std::str::from_utf8(prefix).is_ok(), "precondition: decodes");
        assert!(!prefix.contains(&0), "precondition: no NUL");
        assert!(
            std::str::from_utf8(&bytes).is_err(),
            "precondition: the whole file is not UTF-8"
        );

        assert_eq!(
            detect_mime_prefix(prefix, None).unwrap(),
            "application/octet-stream"
        );
        assert!(
            !is_browser_renderable(&detect_mime_prefix(prefix, None).unwrap()),
            "an unproven prefix must not be advertised as displayable"
        );
        // The same bytes as a whole file legitimately read as text.
        assert_eq!(
            detect_mime_with_hint(prefix, None).unwrap(),
            "text/plain",
            "a whole-file sniff is entitled to the generic answer"
        );
        // And the real file is not text either, by the same token.
        assert_eq!(
            detect_mime_with_hint(&bytes, None).unwrap(),
            "application/octet-stream"
        );
    }

    #[test]
    fn a_truncated_sniff_still_uses_the_name_for_the_generic_end() {
        assert_eq!(
            detect_mime_prefix(b"<svg", Some("logo.svg")).unwrap(),
            "image/svg+xml"
        );
        assert_eq!(
            detect_mime_prefix(b"words", Some("notes.txt")).unwrap(),
            "text/plain"
        );
        // Content still wins when it is specific, even from a prefix.
        assert_eq!(
            detect_mime_prefix(b"\x89PNG\r\n\x1a\n", Some("disguised.html")).unwrap(),
            "image/png"
        );
    }

    #[test]
    fn an_empty_prefix_is_not_an_error() {
        // A zero-byte record is a real thing to open, and the caller is asking
        // what to do with it. The honest answer is "we cannot tell", not a
        // failure that stops the page before it can offer a download.
        assert_eq!(
            detect_mime_prefix(b"", None).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(
            detect_mime_prefix(b"", Some("empty.png")).unwrap(),
            "image/png"
        );
        assert!(!is_browser_renderable("application/octet-stream"));
    }
}

#[cfg(test)]
mod markdown_tests {
    use super::{detect_mime_prefix, detect_mime_with_hint, is_browser_renderable};

    const DOC: &[u8] = b"# Weather log\n\nWind is picking up.\n\n## Tomorrow\n\nMore rain.";

    #[test]
    fn a_heading_opening_the_file_is_recognised_as_markdown() {
        // What the marker can do: `text/markdown` has no signature, so a file
        // that follows the convention is named correctly instead of arriving as
        // an untyped blob.
        assert_eq!(detect_mime_with_hint(DOC, None).unwrap(), "text/markdown");
        assert_eq!(
            detect_mime_prefix(&DOC[..32], None).unwrap(),
            "text/markdown",
            "a marker at the start survives truncation"
        );
    }

    #[test]
    fn front_matter_is_also_a_recognisable_opening() {
        let doc = b"---\ntitle: Weather log\ndate: 2026-01-02\n---\n\nBody text.";
        assert_eq!(detect_mime_with_hint(doc, None).unwrap(), "text/markdown");
    }

    #[test]
    fn a_bom_or_leading_blank_lines_do_not_hide_the_heading() {
        let doc = b"\xef\xbb\xbf\n\n# Weather log\n\nBody.";
        assert_eq!(detect_mime_with_hint(doc, None).unwrap(), "text/markdown");
    }

    #[test]
    fn markdown_recognised_from_content_is_not_pretended_to_be_renderable() {
        // Deliberate. A browser is given `text/markdown` and will usually treat
        // it as an unknown type and download it, so marking it displayable is
        // exactly the blank-window case this policy exists to prevent. What the
        // recognition buys is the *name*: the file is saved as `doc.md` rather
        // than as an untyped blob, which is what was actually missing.
        assert!(!is_browser_renderable("text/markdown"));
    }

    #[test]
    fn prose_without_a_marker_is_still_plain_text() {
        // Recall is partial and that is fine: a markdown file that opens with a
        // paragraph has no marker, and it keeps the `text/plain` answer rather
        // than being guessed at.
        let prose = b"It rained all week, which was expected for April.";
        assert_eq!(detect_mime_with_hint(prose, None).unwrap(), "text/plain");
        // A heading further down is not a marker for the file's type.
        let buried = b"Some intro paragraph.\n\n# A heading much later on";
        assert_eq!(detect_mime_with_hint(buried, None).unwrap(), "text/plain");
    }

    #[test]
    fn a_heading_is_a_positive_marker_not_a_licence_to_guess_binary() {
        // The marker only runs after the bytes have decoded as UTF-8, so it
        // cannot talk a binary into being text.
        let mut binary = b"# ".to_vec();
        binary.extend_from_slice(&[0x9d, 0x00, 0x8f]);
        assert_eq!(
            detect_mime_with_hint(&binary, None).unwrap(),
            "application/octet-stream"
        );
    }
}

#[cfg(test)]
mod naming_tests {
    use super::HINTED_TYPES;
    use crate::filename::extension_for_mime;

    /// Every type we are willing to resolve a name from is a type we can name.
    ///
    /// The two tables answer the same question from opposite directions — "what
    /// does `.csv` mean?" and "what do we call a `text/csv`?" — and they used to
    /// disagree: `image/avif`, `text/csv` and nine others could be *resolved* but
    /// never *named*, so a file the user's own URL identified was downloaded as an
    /// untyped blob. Pairing them is the only way the agreement is maintained,
    /// because a new entry in either list without the other is invisible.
    #[test]
    fn every_type_we_would_resolve_can_also_be_named() {
        for (ext, mime) in HINTED_TYPES {
            assert!(
                extension_for_mime(mime).is_some(),
                "{mime} (resolved from .{ext}) cannot be named"
            );
        }
    }

    #[test]
    fn a_named_type_names_itself_back() {
        // The two directions agreeing is the stronger statement, and it is what
        // makes `.jpg` and `image/jpeg` a pair rather than a coincidence.
        for (_, mime) in HINTED_TYPES {
            let named = extension_for_mime(mime).expect("covered by the test above");
            let round_trips = HINTED_TYPES
                .iter()
                .any(|(e, m)| e.eq_ignore_ascii_case(named) && *m == *mime);
            assert!(
                round_trips,
                "{mime} is named .{named}, which means something else"
            );
        }
    }
}

#[cfg(test)]
mod renderable_tests {
    use super::is_browser_renderable;

    #[test]
    fn the_types_a_browser_can_show_are_renderable() {
        for mime in [
            "text/plain",
            "text/html",
            "text/html; charset=utf-8",
            "text/css",
            "application/json",
            "application/pdf",
            "image/png",
            "image/svg+xml",
            "video/mp4",
            "audio/mpeg",
        ] {
            assert!(is_browser_renderable(mime), "{mime} should be renderable");
        }
    }

    #[test]
    fn anything_we_cannot_display_is_not_renderable() {
        for mime in [
            // The "we could not tell" answer. Rendering it is exactly the blank
            // tab this policy exists to prevent.
            "application/octet-stream",
            "application/zip",
            "application/x-tar",
            "application/octet-stream; charset=binary",
            "",
            "not-a-mime",
            // A bare major type with no subtype is not a type we can display.
            "application",
        ] {
            assert!(
                !is_browser_renderable(mime),
                "{mime} should not be renderable"
            );
        }
    }

    #[test]
    fn parameters_and_case_do_not_change_the_verdict() {
        assert!(is_browser_renderable("TEXT/PLAIN"));
        assert!(is_browser_renderable("text/plain;charset=UTF-8"));
        assert!(!is_browser_renderable("APPLICATION/OCTET-STREAM"));
    }
}
