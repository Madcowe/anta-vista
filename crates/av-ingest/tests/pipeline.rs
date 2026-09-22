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

// ---------------------------------------------------------------------------
// Watch-list bundles
// ---------------------------------------------------------------------------

use av_core::{constants::WATCHLIST_MIME, types::ResourceKind};
use av_ingest::watchlist::apply_tags;
use serde_json::json;

/// Regression: ensure existing resource kinds keep their golden descriptions
/// (no regression in wording).
#[test]
fn golden_sweep_retains_existing_descriptions() {
    // Minimal text file
    let bytes = b"Hello world";
    let desc = ingest_bytes(bytes, Some("readme.txt"), "file:///tmp/readme.txt").unwrap();
    // Expect description contains "text document"
    assert!(desc.description_text.contains("text document"));
    assert_eq!(desc.kind, ResourceKind::Text);
    assert_eq!(desc.mime_type, "text/plain");

    // JPEG
    let jpeg_bytes = [
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01,
        0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
    ];
    let desc = ingest_bytes(&jpeg_bytes, Some("photo.jpg"), "file:///tmp/photo.jpg").unwrap();
    assert!(desc.description_text.contains("image") || desc.description_text.contains("photograph"));
    assert_eq!(desc.kind, ResourceKind::Image);
    assert_eq!(desc.mime_type, "image/jpeg");

    // PDF
    let pdf_bytes = b"%PDF-1.4\n% fake";
    let desc = ingest_bytes(pdf_bytes, Some("doc.pdf"), "file:///tmp/doc.pdf").unwrap();
    assert!(desc.description_text.contains("document"));
    assert_eq!(desc.kind, ResourceKind::Pdf);
    assert_eq!(desc.mime_type, "application/pdf");
}

/// Tag invariant: applying tags to a non-watch‑list prepends “tagged as”.
#[test]
fn tag_invariant_non_watchlist() {
    let mut desc = av_core::types::ResourceDescriptor {
        id: "test".into(),
        kind: ResourceKind::Text,
        location: "file:///tmp/test.txt".into(),
        location_scheme: Some("file".into()),
        location_canonical: None,
        mime_type: "text/plain".into(),
        filename: Some("test.txt".into()),
        metadata_json: json!({}),
        description_text: "a readme text document file in plain format".into(),
        created_at: 0,
    };
    let original = desc.description_text.clone();
    apply_tags(&mut desc, &["urgent".into(), "todo".into()]);
    assert_eq!(
        desc.description_text,
        format!("{} tagged as: urgent, todo", original)
    );
    // Adding empty tags does nothing.
    let before = desc.description_text.clone();
    apply_tags(&mut desc, &["".into()]);
    assert_eq!(desc.description_text, before);
}

/// Tag invariant: applying tags to watch‑list inserts before item list,
/// clamps to EMBED_WINDOW_CHARS.
#[test]
fn tag_invariant_watchlist() {
    let mut desc = av_core::types::ResourceDescriptor {
        id: "test".into(),
        kind: ResourceKind::WatchList,
        location: "file:///tmp/bundle.watch-list".into(),
        location_scheme: Some("file".into()),
        location_canonical: None,
        mime_type: WATCHLIST_MIME.into(),
        filename: Some("bundle.watch-list".into()),
        metadata_json: json!({
            "watch_list": {
                "base": "a bundle media watch list",
                "item_count": 3,
                "media_types": { "mkv": 3 },
                "items": ["Movie1.mkv", "Movie2.mkv", "Movie3.mkv"],
            }
        }),
        description_text: String::new(), // will be overwritten
        created_at: 0,
    };
    apply_tags(&mut desc, &["scifi".into(), "faves".into()]);
    let text = &desc.description_text;
    assert!(text.contains("tagged as: scifi, faves"));
    assert!(text.contains("Movie1")); // at least one item appears
    assert!(text.chars().count() <= 1024); // EMBED_WINDOW_CHARS
}

/// Ensure a ZIP that is not a watch‑list (no datamap/list.txt/hint)
/// does NOT get misclassified as watch‑list.
#[test]
fn false_positive_zip() {
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::SimpleFileOptions};
    use zip::CompressionMethod;

    let mut buf = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(&mut buf));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("readme.txt", opts).unwrap();
    writer.write_all(b"Not a watch list").unwrap();
    writer.finish().unwrap();

    let desc = ingest_bytes(&buf, Some("archive.zip"), "file:///tmp/archive.zip").unwrap();
    assert_ne!(desc.kind, ResourceKind::WatchList);
    assert_ne!(desc.mime_type, WATCHLIST_MIME);
    assert_eq!(desc.kind, ResourceKind::File);
}

/// Determinism: same input yields same description (no randomness).
#[test]
fn description_deterministic() {
    let bytes = b"example content";
    let desc1 = ingest_bytes(bytes, Some("example.txt"), "file:///tmp/example.txt").unwrap();
    let desc2 = ingest_bytes(bytes, Some("example.txt"), "file:///tmp/example.txt").unwrap();
    assert_eq!(desc1.description_text, desc2.description_text);
    assert_eq!(desc1.id, desc2.id);
}

/// Reproduces the reported bug: the flattened inferred filename
/// ("github com aautonomicc watch it raw main catalog public domain.watch-list")
/// must NOT leak into the base description.
#[test]
fn ingest_watchlist_url_uses_real_filename_base() {
    let bundle = {
        use std::io::{Cursor, Write};
        use zip::{ZipWriter, write::SimpleFileOptions};
        use zip::CompressionMethod;

        let mut buf = Vec::new();
        let mut writer = ZipWriter::new(Cursor::new(&mut buf));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer
            .start_file("datamaps/The General (1926).mp4.datamap", opts)
            .unwrap();
        writer.write_all(&[0]).unwrap();
        writer.finish().unwrap();
        buf
    };

    let uri =
        "https://github.com/aautonomicc/Watch-It/raw/main/catalog/Public%20Domain.watch-list";
    let desc = ingest_bytes(&bundle, None, uri).unwrap();
    assert_eq!(desc.kind, ResourceKind::WatchList);
    assert_eq!(desc.mime_type, WATCHLIST_MIME);
    // Base is derived from the last URL segment, not the collapsed path.
    let text = &desc.description_text;
    assert!(
        text.starts_with("a public domain media watch list"),
        "unexpected base: {text}"
    );
    assert!(!text.contains("github"), "host leaked into base: {text}");
    assert!(!text.contains("aautonomicc"), "user leaked into base: {text}");
}

#[test]
fn ingest_watchlist_bundle_smoke() {
    let bundle = {
        use std::io::{Cursor, Write};
        use zip::{ZipWriter, write::SimpleFileOptions};
        use zip::CompressionMethod;

        let mut buf = Vec::new();
        let mut writer = ZipWriter::new(Cursor::new(&mut buf));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("datamaps/My Video (2025) [1080p].mp4.datamap", opts).unwrap();
        writer.write_all(&[0]).unwrap();
        writer.finish().unwrap();
        buf
    };

    let desc = ingest_bytes(
        &bundle,
        Some("My Library.watch-list"),
        "file:///tmp/My Library.watch-list",
    )
    .unwrap();
    assert_eq!(desc.kind, ResourceKind::WatchList);
    assert_eq!(desc.mime_type, WATCHLIST_MIME);
    assert!(desc.description_text.contains("watch list"));
    assert!(
        desc.description_text.contains("My Video (2025) [1080p]")
            || desc.description_text.contains("more")
    );
}

/// Realistic bundle: a series spread over many episodes plus standalone movies
/// and a hex-suffixed duplicate. Episodes must collapse to a range, movies must
/// stay, duplicates must dedupe, hex junk must not leak.
#[test]
fn ingest_watchlist_compacts_series_and_dedupes_movies() {
    let bundle = {
        use std::io::{Cursor, Write};
        use zip::{ZipWriter, write::SimpleFileOptions};
        use zip::CompressionMethod;

        let mut buf = Vec::new();
        let mut writer = ZipWriter::new(Cursor::new(&mut buf));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        for ep in 1..=15u32 {
            let name = format!("datamaps/Petticoat Junction S01E{ep:02}.mp4.datamap");
            writer.start_file(name, opts).unwrap();
            writer.write_all(&[0]).unwrap();
        }
        for name in [
            "datamaps/The General (1926) {imdb-tt0017925}.mp4.datamap",
            "datamaps/Nosferatu (1922) {imdb-tt0013442}.mp4.datamap",
            "datamaps/Night of the Living Dead (1968) {imdb-tt0063350} - [1080p].mp4.datamap",
            "datamaps/Night of the Living Dead (1968) {imdb-tt0063350} - [1080p].mp4.66cacd06.datamap",
        ] {
            writer.start_file(name, opts).unwrap();
            writer.write_all(&[0]).unwrap();
        }

        writer.finish().unwrap();
        buf
    };

    let desc = ingest_bytes(
        &bundle,
        Some("Public Domain.watch-list"),
        "https://github.com/aautonomicc/Watch-It/raw/main/catalog/Public%20Domain.watch-list",
    )
    .unwrap();

    let text = &desc.description_text;
    // Episodes collapsed into one range.
    assert!(
        text.contains("Petticoat Junction season 1 episodes 1-15"),
        "episodes not collapsed: {text}"
    );
    // Movies still listed.
    assert!(text.contains("The General (1926)"), "movie missing: {text}");
    assert!(text.contains("Nosferatu (1922)"), "movie missing: {text}");
    // Duplicate movie deduped to a single entry.
    let living_dead = text.matches("Night of the Living Dead").count();
    assert_eq!(living_dead, 1, "duplicate movie not deduped: {text}");
    // No hex junk / no .mp4 extensions leaking.
    assert!(!text.contains("66cacd06"), "hex junk leaked: {text}");
    assert!(!text.contains(".mp4"), "extensions leaked: {text}");
}
