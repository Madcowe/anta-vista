//! Local semantic search example.
//!
//! Demonstrates the full anta-vista pipeline:
//!   1. Ingest content (bytes → ResourceDescriptor with description)
//!   2. Embed descriptions (mock provider — no model download needed)
//!   3. Store in SQLite (in-memory)
//!   4. Search by natural language query
//!   5. Print ranked results with score breakdown
//!
//! Run with:
//!   cargo run --example local_search -p anta-vista-examples
//!
//! To use the real MiniLM model (downloads ~22 MB on first run):
//!   Swap MockEmbeddingProvider for MiniLmProvider (see comment in main).
//!   Requires internet access on first use.

use av_core::types::{NameRecord, NameRecordType, normalize_name};
use av_embed::{
    mock::MockEmbeddingProvider,
    provider::{EmbeddingProvider, profile_id},
};
use av_index::{LocalIndex, QueryFilter, SchemeFilter};
use av_ingest::ingest_bytes;
use av_store::{open_in_memory, repo};

// Sample content — (filename, bytes, location)
const SAMPLES: &[(&str, &[u8], &str)] = &[
    (
        "fish.jpg",
        &[
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
        ],
        "https://example.com/fish.jpg",
    ),
    (
        "rust_guide.txt",
        b"The Rust programming language provides memory safety without garbage collection.",
        "https://example.com/rust_guide.txt",
    ),
    (
        "cheesy.mp3",
        &[0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        "https://example.com/cheesy.mp3",
    ),
    (
        "autonomi_index.txt",
        b"Autonomi network distributed storage system index file.",
        "ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03",
    ),
    (
        "report.pdf",
        b"%PDF-1.4\n1 0 obj\n<</Type /Catalog>>\nendobj\n%%EOF",
        "https://example.com/report.pdf",
    ),
];

fn main() {
    // Initialise logging (RUST_LOG=info to see details)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  anta-vista — local search example");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Open in-memory SQLite database
    let conn = open_in_memory().expect("failed to open in-memory database");

    // Use mock provider (deterministic, no download)
    // For real MiniLM: use av_embed::MiniLmProvider::new().expect("load model")
    let provider = MockEmbeddingProvider::new();
    let pid = profile_id(provider.profile());

    // Register the embedding profile
    repo::embeddings::insert_profile(&conn, &pid, provider.profile()).expect("insert profile");

    // ── Ingest and index all sample documents ─────────────────────────────

    println!("\n📥  Ingesting {} documents...\n", SAMPLES.len());

    for (filename, bytes, location) in SAMPLES {
        let resource = ingest_bytes(bytes, Some(filename), location).expect("ingest failed");

        println!(
            "  [{:>20}]  {}  →  \"{}\"",
            resource.mime_type, filename, resource.description_text
        );

        // Store resource
        repo::resources::insert(&conn, &resource).expect("store resource");

        // Embed and store
        let embedding = provider
            .embed_resource(&resource.description_text, &resource.id)
            .expect("embed failed");
        repo::embeddings::insert(&conn, &embedding).expect("store embedding");
    }

    // ── Register a name record (DNS-like) ─────────────────────────────────

    let name_record = NameRecord {
        schema_version: 1,
        record_id: "example-name-001".to_string(),
        normalized_name: normalize_name("Autonomi.Index"),
        original_name: "Autonomi.Index".to_string(),
        record_type: NameRecordType::Uri,
        target: "ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03"
            .to_string(),
        target_scheme: Some("ant".to_string()),
        target_canonical: Some(
            "ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03".to_string(),
        ),
        ttl_secs: 3600,
        by_agent_id: "local-agent".to_string(),
        timestamp: 1_700_000_000,
        signature: vec![],
    };
    repo::names::insert(&conn, &name_record).expect("store name record");

    // ── Semantic search ───────────────────────────────────────────────────

    let index = LocalIndex::new(&conn, &provider);

    let queries = [
        ("image or photo", QueryFilter::default()),
        ("music audio file", QueryFilter::default()),
        ("programming document", QueryFilter::default()),
        ("distributed network storage", QueryFilter::default()),
    ];

    for (query, filter) in &queries {
        println!("\n🔍  Query: \"{}\"", query);
        println!("    {:-<53}", "");

        let results = index.search(query, 3, filter).expect("search failed");

        if results.is_empty() {
            println!("    (no results)");
        } else {
            for (i, r) in results.iter().enumerate() {
                println!(
                    "    {}. [score {:.3}] {} — {}",
                    i + 1,
                    r.score,
                    r.resource.description_text,
                    r.resource.location
                );
                println!(
                    "       semantic={:.3}  agreement={:.3}  feedback={:.3}  trust={:.3}",
                    r.semantic_score, r.agreement_score, r.feedback_score, r.trust_score
                );
            }
        }
    }

    // ── Scheme-filtered search ────────────────────────────────────────────

    println!("\n🔍  Query: \"file\" (ant:// only)");
    println!("    {:-<53}", "");
    let mut ant_filter = QueryFilter::default();
    ant_filter.scheme = SchemeFilter::new(["ant"]);
    let ant_results = index.search("file", 5, &ant_filter).expect("search failed");
    if ant_results.is_empty() {
        println!("    (no ant:// resources found)");
    } else {
        for r in &ant_results {
            println!(
                "    → {} [{}]",
                r.resource.description_text, r.resource.location
            );
        }
    }

    // ── Name resolution ───────────────────────────────────────────────────

    println!("\n🏷️   Name resolution: \"autonomi.index\" (case-insensitive)");
    println!("    {:-<53}", "");
    let name_results = index
        .resolve_name("AUTONOMI.INDEX", &SchemeFilter::default())
        .expect("resolve failed");
    if name_results.is_empty() {
        println!("    (not found)");
    } else {
        for r in &name_results {
            println!(
                "    → {} [score {:.3}, ttl_valid={}]",
                r.record.target, r.score, r.ttl_valid
            );
        }
    }

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "  Done. All {} documents indexed and searched.",
        SAMPLES.len()
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
}
