use crate::{
    describe::synthesize,
    error::IngestResult,
    location::analyze_location,
    metadata::extract,
    mime::{canonicalize_mime, detect_mime, mime_major},
};
use av_core::types::{ResourceDescriptor, ResourceKind};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Ingest a resource from raw bytes + optional filename/path.
/// Returns a fully populated `ResourceDescriptor`.
pub fn ingest_bytes(
    bytes: &[u8],
    filename: Option<&str>,
    location: &str,
) -> IngestResult<ResourceDescriptor> {
    let mime_raw = detect_mime(bytes)?;
    let mime = canonicalize_mime(&mime_raw);

    let kind = kind_from_mime(&mime);

    let location_info = analyze_location(location);
    let location_scheme = location_info.scheme;
    let location_canonical = location_info.canonical;
    let inferred_filename = location_info.inferred_filename;

    let is_autonomi = location_scheme.as_deref() == Some("autonomi");
    let resource_filename = filename
        .map(|f| f.to_string())
        .or_else(|| {
            if is_autonomi {
                inferred_filename.clone()
            } else {
                None
            }
        });

    let effective_filename = filename.or(inferred_filename.as_deref());

    let meta = extract(bytes, &mime);

    let description_text = synthesize(&mime, effective_filename, &meta);

    // Build metadata JSON
    let metadata_json = meta.extra.clone();

    // Resource ID: SHA-256 of bytes
    let id = {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    };

    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    Ok(ResourceDescriptor {
        id,
        kind,
        location: location.to_string(),
        location_scheme,
        location_canonical,
        mime_type: mime,
        filename: resource_filename,
        metadata_json,
        description_text,
        created_at,
    })
}

/// Ingest a resource from a file path on disk.
pub fn ingest_file(path: &Path) -> IngestResult<ResourceDescriptor> {
    let bytes = std::fs::read(path)?;
    let filename = path.file_name().and_then(|n| n.to_str());
    let location = format!("file://{}", path.display());
    ingest_bytes(&bytes, filename, &location)
}

fn kind_from_mime(mime: &str) -> ResourceKind {
    match mime_major(mime) {
        "image" => ResourceKind::Image,
        "audio" => ResourceKind::Audio,
        "video" => ResourceKind::File,
        "text" => ResourceKind::Text,
        "application" => match mime {
            "application/pdf" => ResourceKind::Pdf,
            _ => ResourceKind::File,
        },
        _ => ResourceKind::File,
    }
}
