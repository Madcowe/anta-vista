use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cmd::{CliError, CliResult};
use crate::download::verify_uri_exists;
use crate::output::print_output;
use crate::startup::StartupState;
use av_core::types::{normalize_name, NameRecord, NameRecordType};
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::dispatcher::MessageDispatcher;
use dialoguer::Confirm;
use serde_json::json;
use uuid::Uuid;

pub fn run(
    cli: crate::Cli,
    state: StartupState,
    uri: String,
    name: String,
    record_type: String,
    ttl: u32,
    no_verify: bool,
) -> CliResult<()> {
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    // Verify URI reachability unless skipped
    let verified = if no_verify {
        false
    } else {
        match verify_uri_exists(&uri) {
            Ok(()) => true,
            Err(e) => {
                if cli.non_interactive {
                    let err =
                        json!({"ok": false, "error": "uri_unreachable", "detail": e.to_string()});
                    println!("{}", serde_json::to_string_pretty(&err).unwrap());
                    return Ok(());
                } else {
                    println!(
                        "{} URI appears unreachable: {}",
                        console::style("⚠").yellow(),
                        e
                    );
                    let proceed = Confirm::new()
                        .with_prompt("Register anyway?")
                        .default(false)
                        .interact()
                        .map_err(|e| CliError::Other(e.to_string()))?;
                    if !proceed {
                        return Err(CliError::Validation("Aborted by user".to_string()));
                    }
                    false
                }
            }
        }
    };

    // Parse record type string
    let parsed_type = match record_type.as_str() {
        "a" => NameRecordType::A,
        "txt" => NameRecordType::Txt,
        "uri" => NameRecordType::Uri,
        "service" => NameRecordType::Service,
        other => {
            return Err(CliError::Validation(format!(
                "Unknown record type: {}",
                other
            )));
        }
    };

    // Build the NameRecord
    let agent_id = state
        .x0x_config
        .as_ref()
        .map(|c| c.agent_id.clone())
        .unwrap_or_else(|| "local".to_string());

    let location_info = av_ingest::location::analyze_location(&uri);
    let target_scheme = location_info.scheme.clone();
    let target_canonical = location_info.canonical.clone();

    let normalized = normalize_name(&name);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Check if we are updating an existing record
    let existing = av_store::repo::names::get_by_normalized_name(&conn, &normalized)
        .map_err(|e| CliError::Database(e.to_string()))?;
    let updated_existing = existing.iter().any(|r| r.by_agent_id == agent_id);

    let record = NameRecord {
        schema_version: 1,
        record_id: Uuid::new_v4().to_string(),
        normalized_name: normalized.clone(),
        original_name: name.clone(),
        record_type: parsed_type,
        target: uri.clone(),
        target_scheme,
        target_canonical,
        ttl_secs: ttl,
        by_agent_id: agent_id,
        timestamp,
        signature: vec![], // Signature would require x0x signing — placeholder for MVP
    };

    // Store locally
    av_store::repo::names::insert(&conn, &record).map_err(|e| CliError::Database(e.to_string()))?;

    // Broadcast via gossip if x0x is running
    let broadcast = if let Some(ref x0x_cfg) = state.x0x_config {
        let net_client = Arc::new(X0xNetClient::new(x0x_cfg.clone()));
        let dispatcher = MessageDispatcher::new(net_client);
        // Subscribe to name topics so we receive any conflicting claims in return.
        let _ = dispatcher.subscribe_all();
        match dispatcher.publish_name_claim(record.clone()) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Failed to broadcast name claim: {}", e);
                false
            }
        }
    } else {
        false
    };

    let output_json = json!({
        "ok": true,
        "record_id": record.record_id,
        "name": name,
        "normalized_name": normalized,
        "target": uri,
        "record_type": record_type,
        "ttl_secs": ttl,
        "verified": verified,
        "updated_existing": updated_existing,
        "broadcast": broadcast,
    });

    print_output(
        cli.non_interactive,
        || {
            println!(
                "Registering name {} → {}",
                console::style(&name).cyan().bold(),
                console::style(&uri).green()
            );
            if verified {
                println!("  {} URI verified", console::style("✓").green());
            } else {
                println!(
                    "  {} URI not verified (skipped)",
                    console::style("~").yellow()
                );
            }
            println!(
                "  {} Name record stored locally",
                console::style("✓").green()
            );
            if broadcast {
                println!(
                    "  {} Claim broadcast to network",
                    console::style("✓").green()
                );
            } else {
                println!(
                    "  {} Not broadcast (x0x offline)",
                    console::style("~").yellow()
                );
            }
            if updated_existing {
                println!("\n  Note: Updated an existing record for this name.");
            }
        },
        &output_json,
    );

    Ok(())
}
