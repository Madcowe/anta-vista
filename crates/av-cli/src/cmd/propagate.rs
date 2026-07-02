use crate::cmd::{CliError, CliResult};
use crate::startup::StartupState;
use av_core::types::{EmbeddingRecord, ResourceDescriptor, ResourceKind};
use av_embed::minilm::MiniLmProvider;
use av_embed::provider::EmbeddingProvider;
use rusqlite::Connection;

pub fn run(
    cli: crate::Cli,
    _state: StartupState,
    resource_id: String,
    location: String,
    description: String,
    mime_type: Option<String>,
) -> CliResult<()> {
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    if av_store::repo::resources::get(&conn, &resource_id)
        .map_err(|e| CliError::Database(e.to_string()))?
        .is_some()
    {
        println!("  Resource already indexed locally");
        return Ok(());
    }

    if description.is_empty() {
        return Err(CliError::Validation(
            "Description cannot be empty".to_string(),
        ));
    }

    let provider = MiniLmProvider::new().map_err(|e| CliError::Model(e.to_string()))?;
    let mime = mime_type.unwrap_or_else(|| "application/octet-stream".to_string());

    propagate_resource(&conn, &provider, &resource_id, &location, &description, &mime)?;

    if cli.non_interactive {
        println!(
            "{}",
            serde_json::json!({ "status": "propagated", "resource_id": resource_id })
        );
    } else {
        println!(
            "  {} Propagated {} to local index",
            console::style("✓").green(),
            resource_id,
        );
    }
    Ok(())
}

/// Re-embed a resource's description and store it in the local index.
///
/// Used by both the `av propagate` command and the interactive relevance
/// feedback prompt in `av search`. A no-op if the resource already exists.
pub fn propagate_resource(
    conn: &Connection,
    provider: &impl EmbeddingProvider,
    resource_id: &str,
    location: &str,
    description: &str,
    mime_type: &str,
) -> CliResult<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let embedding = provider
        .embed_text(description)
        .map_err(|e| CliError::Model(e.to_string()))?;

    let profile = provider.profile();
    let pid = av_embed::provider::profile_id(&profile);

    av_store::repo::embeddings::insert_profile(conn, &pid, &profile)
        .map_err(|e| CliError::Database(e.to_string()))?;

    let l2_norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();

    let resource = ResourceDescriptor {
        id: resource_id.to_string(),
        kind: kind_from_mime(mime_type),
        location: location.to_string(),
        location_scheme: None,
        location_canonical: None,
        mime_type: mime_type.to_string(),
        filename: None,
        metadata_json: serde_json::json!({
            "propagated": true,
            "propagated_at": now,
        }),
        description_text: description.to_string(),
        created_at: now,
    };

    av_store::repo::resources::insert(conn, &resource)
        .map_err(|e| CliError::Database(e.to_string()))?;

    av_store::repo::embeddings::insert(
        conn,
        &EmbeddingRecord {
            resource_id: resource_id.to_string(),
            profile_id: pid,
            vector: embedding,
            l2_norm,
            created_at: now,
        },
    )
    .map_err(|e| CliError::Database(e.to_string()))?;

    Ok(())
}

fn kind_from_mime(mime: &str) -> ResourceKind {
    match mime.split('/').next().unwrap_or("") {
        "text" => ResourceKind::Text,
        "image" => ResourceKind::Image,
        "audio" => ResourceKind::Audio,
        _ if mime.contains("pdf") => ResourceKind::Pdf,
        _ => ResourceKind::File,
    }
}
