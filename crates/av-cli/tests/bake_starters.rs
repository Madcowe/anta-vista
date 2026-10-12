//! Regenerates `src/starters_data.rs` — the baked starter descriptors and
//! their MiniLM vectors.
//!
//! Needs the network (two live pages) and the real model (~87 MB on first
//! run), so it is `#[ignore]`d. Re-run it whenever the pages' content should
//! be refreshed, then commit the regenerated file:
//!
//! ```text
//! cargo test -p av-cli --test bake_starters -- --ignored --nocapture
//! ```
//!
//! The Autonomi starter is never fetched: its address is content-immutable
//! and a fresh install has no backend to fetch it with, so its descriptor is
//! built from the location and its `dog` tag alone — exactly what
//! [`av_cli::starters::ensure_seeded`] will insert.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use av_cli::download::{DownloadOptions, download_content};
use av_core::types::{ResourceDescriptor, ResourceKind};
use av_embed::minilm::MiniLmProvider;
use av_embed::provider::EmbeddingProvider;
use av_ingest::describe::synthesize;
use av_ingest::location::{analyze_location, normalize_uri};
use av_ingest::metadata::ExtractedMeta;
use av_ingest::{apply_tags, ingest_bytes};
use sha2::Digest;

/// The live pages to bake, in the order they appear in the generated file.
const PAGES: &[&str] = &["https://autonomi.com/", "https://x0x.md/"];

/// The Autonomi record: seeded from location + tags alone, never fetched.
const LUCKY_LOCATION: &str =
    "autonomi://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a/lucky.jpg";
const LUCKY_TAGS: &[&str] = &["dog"];

#[test]
#[ignore = "network + real model; regenerates src/starters_data.rs"]
fn bake_starters() {
    let provider = MiniLmProvider::new().expect("load MiniLM");
    let mut entries: Vec<(ResourceDescriptor, Vec<f32>)> = Vec::new();

    for url in PAGES {
        let normalized = normalize_uri(url);
        let bytes = download_content(&normalized, None, DownloadOptions::default())
            .unwrap_or_else(|e| panic!("fetch {normalized}: {e}"));
        let resource = ingest_bytes(&bytes, None, &normalized).expect("ingest");
        let vector = provider
            .embed_text(&resource.description_text)
            .expect("embed description");
        eprintln!(
            "baked {normalized} -> {} ({} chars of description)",
            resource.id,
            resource.description_text.len()
        );
        entries.push((resource, vector));
    }

    entries.push(bake_lucky(&provider));
    write_data_file(&entries).expect("write starters_data.rs");
}

/// Build the Autonomi starter the way a user's own `av index` would have,
/// minus the fetch: no bytes, so the id derives from the location (ingest's
/// ids are sha256 of the content, and there is no content until someone
/// opens it), and the description is synthesized from the filename and tag.
fn bake_lucky(provider: &MiniLmProvider) -> (ResourceDescriptor, Vec<f32>) {
    let location = normalize_uri(LUCKY_LOCATION);
    let info = analyze_location(&location);
    let meta = ExtractedMeta::default();

    let mut resource = ResourceDescriptor {
        id: hex::encode(sha2::Sha256::digest(location.as_bytes())),
        kind: ResourceKind::Image,
        location: location.clone(),
        location_scheme: info.scheme,
        location_canonical: info.canonical,
        mime_type: "image/jpeg".to_string(),
        filename: info.inferred_filename,
        metadata_json: serde_json::json!({}),
        description_text: synthesize(
            "image/jpeg",
            analyze_location(&location).inferred_filename.as_deref(),
            &meta,
        ),
        created_at: now_secs(),
    };
    let tags: Vec<String> = LUCKY_TAGS.iter().map(|t| t.to_string()).collect();
    apply_tags(&mut resource, &tags);

    let vector = provider
        .embed_text(&resource.description_text)
        .expect("embed description");
    eprintln!(
        "baked {location} -> {} ({:?})",
        resource.id, resource.description_text
    );
    (resource, vector)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Write `src/starters_data.rs`.
///
/// String literals are emitted through `{:?}` so every escape is a valid Rust
/// literal — page content can contain quotes and backslashes, and hand-rolled
/// quoting is how a generated file stops compiling.
fn write_data_file(entries: &[(ResourceDescriptor, Vec<f32>)]) -> std::io::Result<()> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/starters_data.rs");
    let mut out = String::new();

    (|| -> std::fmt::Result {
        writeln!(out, "//! GENERATED FILE — do not edit by hand.")?;
        writeln!(out, "//!")?;
        writeln!(
            out,
            "//! Regenerate with `cargo test -p av-cli --test bake_starters -- --ignored --nocapture`."
        )?;
        writeln!(
            out,
            "//! Baked from the live sources; each descriptor's `created_at` is the bake time."
        )?;
        writeln!(out, "//! Sources:")?;
        for (r, _) in entries {
            writeln!(out, "//!   {} — {:?}", r.location, r.description_text)?;
        }
        writeln!(out, "//!")?;
        writeln!(
            out,
            "//! Vectors are those descriptions embedded with all-MiniLM-L6-v2"
        )?;
        writeln!(
            out,
            "//! (profile all-MiniLM-L6-v2:v1), the profile searches query against."
        )?;
        writeln!(out)?;
        writeln!(out, "use crate::starters::BakedStarter;")?;
        writeln!(out)?;
        writeln!(out, "pub const STARTERS: &[BakedStarter] = &[")?;
        for (i, (r, _)) in entries.iter().enumerate() {
            let json = serde_json::to_string(r).expect("serialize descriptor");
            writeln!(out, "    BakedStarter {{")?;
            writeln!(out, "        resource_json: {json:?},")?;
            writeln!(out, "        vector: &V{i},")?;
            writeln!(out, "    }},")?;
        }
        writeln!(out, "];")?;
        writeln!(out)?;
        for (i, (_, v)) in entries.iter().enumerate() {
            writeln!(out, "const V{i}: [f32; {}] = [", v.len())?;
            for chunk in v.chunks(6) {
                let line: Vec<String> = chunk.iter().map(|x| format!("{x:?}")).collect();
                writeln!(out, "    {},", line.join(", "))?;
            }
            writeln!(out, "];")?;
            writeln!(out)?;
        }
        Ok(())
    })()
    .expect("format generated file");

    std::fs::write(&path, &out)?;
    eprintln!("wrote {} ({} bytes)", path.display(), out.len());
    Ok(())
}
