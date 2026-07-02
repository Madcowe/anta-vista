use av_ingest::{
    describe::synthesize, filename::tokenize_filename_opt, ingest_bytes, metadata::ExtractedMeta,
    mime::detect_mime,
};

// Minimal valid JPEG: starts with FF D8 FF E0 (JFIF APP0 marker)
const MINIMAL_JPEG: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01,
    0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
];

// Minimal valid PNG: PNG magic bytes + IHDR chunk start
const MINIMAL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
];

// Minimal PDF header — %PDF magic is sufficient for `infer`
const MINIMAL_PDF: &[u8] =
    b"%PDF-1.4\n%%EOF\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

// Plain text
const PLAIN_TEXT: &[u8] = b"Hello world\nThis is a test file\n";

// ID3v2 header (minimal — `infer` recognises "ID3" magic as audio/mpeg)
const MINIMAL_ID3: &[u8] = &[
    0x49, 0x44, 0x33, // "ID3"
    0x04, 0x00, 0x00, // version 2.4, no flags
    0x00, 0x00, 0x00, 0x00, // syncsafe size = 0 (no frames)
];

// ---------------------------------------------------------------------------
// Filename tokenization
// ---------------------------------------------------------------------------

#[test]
fn test_filename_tokenization_fish() {
    assert_eq!(tokenize_filename_opt("fish.jpg"), Some("fish".to_string()));
}

#[test]
fn test_filename_tokenization_cheesy() {
    assert_eq!(
        tokenize_filename_opt("cheesy.mp3"),
        Some("cheesy".to_string())
    );
}

#[test]
fn test_filename_tokenization_compound() {
    // "cheesy_fish-01.jpg" -> "cheesy fish" (drops numeric-only token "01")
    assert_eq!(
        tokenize_filename_opt("cheesy_fish-01.jpg"),
        Some("cheesy fish".to_string())
    );
}

// ---------------------------------------------------------------------------
// Description synthesis
// ---------------------------------------------------------------------------

#[test]
fn test_description_fish_jpeg() {
    let meta = ExtractedMeta::default();
    let desc = synthesize("image/jpeg", Some("fish.jpg"), &meta);
    assert_eq!(desc, "a fish image file in jpeg format");
}

#[test]
fn test_description_cheesy_mp3() {
    let meta = ExtractedMeta::default();
    let desc = synthesize("audio/mpeg", Some("cheesy.mp3"), &meta);
    assert_eq!(desc, "a cheesy music audio file");
}

#[test]
fn test_description_unknown_jpeg() {
    // No filename → fallback to "photograph or image" label + "file" suffix
    let meta = ExtractedMeta::default();
    let desc = synthesize("image/jpeg", None, &meta);
    assert_eq!(desc, "a photograph or image file");
}

#[test]
fn test_description_readme_txt() {
    let meta = ExtractedMeta::default();
    let desc = synthesize("text/plain", Some("readme.txt"), &meta);
    assert_eq!(desc, "a readme text document file in plain format");
}

#[test]
fn test_description_with_metadata_title_no_filename() {
    let mut meta = ExtractedMeta::default();
    meta.title = Some("My Annual Report".to_string());
    let desc = synthesize("application/pdf", None, &meta);
    assert_eq!(desc, "a document or report titled \"My Annual Report\"");
}

// ---------------------------------------------------------------------------
// MIME detection
// ---------------------------------------------------------------------------

#[test]
fn test_mime_detect_jpeg() {
    let mime = detect_mime(MINIMAL_JPEG).unwrap();
    assert_eq!(mime, "image/jpeg");
}

#[test]
fn test_mime_detect_png() {
    let mime = detect_mime(MINIMAL_PNG).unwrap();
    assert_eq!(mime, "image/png");
}

#[test]
fn test_mime_detect_pdf() {
    let mime = detect_mime(MINIMAL_PDF).unwrap();
    assert_eq!(mime, "application/pdf");
}

#[test]
fn test_mime_detect_text() {
    let mime = detect_mime(PLAIN_TEXT).unwrap();
    assert_eq!(mime, "text/plain");
}

#[test]
fn test_mime_detect_empty_error() {
    use av_ingest::IngestError;
    let result = detect_mime(&[]);
    assert!(matches!(result, Err(IngestError::FileTooSmall)));
}

// ---------------------------------------------------------------------------
// Full pipeline — ingest_bytes
// ---------------------------------------------------------------------------

#[test]
fn test_ingest_bytes_jpeg() {
    let resource = ingest_bytes(MINIMAL_JPEG, Some("fish.jpg"), "file:///tmp/fish.jpg").unwrap();
    assert_eq!(resource.mime_type, "image/jpeg");
    assert_eq!(
        resource.description_text,
        "a fish image file in jpeg format"
    );
    assert_eq!(resource.filename.as_deref(), Some("fish.jpg"));
    assert!(!resource.id.is_empty());
    assert_eq!(resource.id.len(), 64); // SHA-256 hex = 64 chars
    assert_eq!(resource.location_scheme.as_deref(), Some("file"));
}

#[test]
fn test_ingest_bytes_text() {
    let resource = ingest_bytes(
        PLAIN_TEXT,
        Some("readme.txt"),
        "https://example.com/readme.txt",
    )
    .unwrap();
    assert_eq!(resource.mime_type, "text/plain");
    assert_eq!(resource.location_scheme.as_deref(), Some("https"));
}

#[test]
fn test_ingest_bytes_no_location_scheme() {
    // A bare path without "://" should produce no location_scheme
    let resource = ingest_bytes(PLAIN_TEXT, None, "/tmp/data").unwrap();
    assert!(resource.location_scheme.is_none());
}

#[test]
fn test_ingest_infers_ant_path_filename() {
    let location =
        "ant://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a/lucky.jpg";
    let resource = ingest_bytes(MINIMAL_JPEG, None, location).unwrap();
    assert_eq!(resource.location_scheme.as_deref(), Some("autonomi"));
    assert_eq!(
        resource.location_canonical.as_deref(),
        Some("autonomi://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a")
    );
    assert_eq!(resource.filename.as_deref(), Some("lucky.jpg"));
    assert_eq!(
        resource.description_text,
        "a lucky image file in jpeg format"
    );
}

#[test]
fn test_ingest_infers_ant_query_name_filename() {
    let location =
        "ant://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a?name=lucky.jpg";
    let resource = ingest_bytes(MINIMAL_JPEG, None, location).unwrap();
    assert_eq!(resource.filename.as_deref(), Some("lucky.jpg"));
    assert_eq!(
        resource.location_canonical.as_deref(),
        Some("autonomi://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a")
    );
}

#[test]
fn test_ingest_infers_autonomi_query_name_filename() {
    let location = "autonomi://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a?name=lucky.jpg";
    let resource = ingest_bytes(MINIMAL_JPEG, None, location).unwrap();
    assert_eq!(resource.location_scheme.as_deref(), Some("autonomi"));
    assert_eq!(resource.filename.as_deref(), Some("lucky.jpg"));
    assert_eq!(
        resource.location_canonical.as_deref(),
        Some("autonomi://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a")
    );
}

#[test]
fn test_ingest_explicit_filename_overrides_uri_hint() {
    let location =
        "ant://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a?name=lucky.jpg";
    let resource = ingest_bytes(MINIMAL_JPEG, Some("chosen.jpg"), location).unwrap();
    assert_eq!(resource.filename.as_deref(), Some("chosen.jpg"));
    assert_eq!(
        resource.description_text,
        "a chosen image file in jpeg format"
    );
}

#[test]
fn test_ingest_ant_path_filename_overrides_query_name() {
    let location = "ant://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a/path.jpg?name=query.jpg";
    let resource = ingest_bytes(MINIMAL_JPEG, None, location).unwrap();
    assert_eq!(resource.filename.as_deref(), Some("path.jpg"));
}

#[test]
fn test_ingest_ignores_empty_or_unsafe_query_name() {
    let empty = ingest_bytes(
        MINIMAL_JPEG,
        None,
        "ant://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a?name=",
    )
    .unwrap();
    assert_eq!(empty.filename, None);

    let unsafe_name = ingest_bytes(
        MINIMAL_JPEG,
        None,
        "ant://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a?name=../secret.jpg",
    )
    .unwrap();
    assert_eq!(unsafe_name.filename, None);
}

#[test]
fn test_ingest_non_ant_location_does_not_infer_filename() {
    let resource = ingest_bytes(MINIMAL_JPEG, None, "https://example.com/lucky.jpg").unwrap();
    assert_eq!(resource.location_scheme.as_deref(), Some("https"));
    assert_eq!(resource.filename, None);
    assert_eq!(
        resource.location_canonical.as_deref(),
        Some("https://example.com/lucky.jpg")
    );
}

#[test]
fn test_ingest_sha256_deterministic() {
    let r1 = ingest_bytes(MINIMAL_JPEG, Some("a.jpg"), "file:///a.jpg").unwrap();
    let r2 = ingest_bytes(MINIMAL_JPEG, Some("b.jpg"), "file:///b.jpg").unwrap();
    // Same bytes → same SHA-256 id regardless of filename/location
    assert_eq!(r1.id, r2.id);
}
