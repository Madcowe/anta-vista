use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use av_core::types::EmbeddingRecord;
use av_embed::minilm::MiniLmProvider;
use av_embed::provider::{EmbeddingProvider, profile_id};
use av_ingest::ingest::ingest_bytes;
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::ResourceResult;
use dialoguer::{Confirm, Input};
use indicatif::{ProgressBar, ProgressStyle};
use rusqlite::Connection;
use serde_json::json;

use crate::cmd::{CliError, CliResult};
use crate::download::{DownloadEvent, DownloadOptions, download_content, verify_uri_exists};
use crate::output::{abbrev, print_output};
use crate::startup::{
    StartupState, ant_cli_binary_available, install_ant_cli, refresh_ant_path, start_antd_daemon,
};

/// Result of indexing one URI.
pub struct IndexOutcome {
    pub resource_id: String,
    pub location: String,
    pub mime_type: String,
    pub kind: String,
    pub description: String,
    pub embedding_dim: usize,
    /// Size of the fetched content in bytes, or 0 when nothing was downloaded
    /// (a duplicate, or `--no-download`).
    pub bytes_len: usize,
    /// True when the URI was already indexed and nothing was written.
    pub duplicate: bool,
    /// Which dedup rule matched: the same location, or the same content at a
    /// different location.
    pub duplicate_by: Option<&'static str>,
    pub broadcast: bool,
}

/// Fetch a URI, then hand the bytes to the shared indexing pipeline.
///
/// This is the whole of `av index` minus the interactive prompts, so the web
/// interface can index a link without reimplementing the pipeline. Never
/// prompts and never exits; every failure is returned.
// Nine parameters is a lot, but they are all distinct switches of one pipeline
// (where to store, what to store, how hard to try, and how to report progress),
// and grouping them would mean inventing a config type used in exactly one place.
#[allow(clippy::too_many_arguments)]
pub fn index_resource(
    state: &StartupState,
    conn: &Connection,
    provider: &MiniLmProvider,
    uri: &str,
    tags: &[String],
    no_download: bool,
    no_verify: bool,
    force: bool,
    on_progress: Option<&dyn Fn(DownloadEvent)>,
) -> CliResult<IndexOutcome> {
    // --- Step 1: Verify URI (unless skipped) --------------------------------
    // Checked before that step deliberately: a manifest link's bytes travel in
    // the link itself, and the reachability check below would wrap this
    // precise error in a generic "URI unreachable".
    if av_ingest::location::is_manifest_link(uri) {
        return Err(CliError::Validation(
            av_ingest::location::manifest_link_error(uri),
        ));
    }
    // Only this path checks, because it is the one that has to reach the resource
    // before it can fetch it. A caller already holding the bytes has, by
    // definition, already reached it.
    if !no_verify {
        verify_uri_exists(uri).map_err(|e| CliError::Network(format!("URI unreachable: {e}")))?;
    }

    // --- Step 2: Download content (unless skipped) --------------------------
    let bytes = if no_download {
        // Use the URI as a 1-byte placeholder so ingest can derive location info
        uri.as_bytes().to_vec()
    } else {
        let progress = |event: DownloadEvent| {
            if let Some(cb) = on_progress {
                cb(event)
            }
        };
        download_content(uri, Some(&progress), DownloadOptions::default())?
    };

    index_resource_from_bytes(state, conn, provider, &bytes, uri, tags, force)
}

/// Ingest, embed and store bytes the caller already holds, then announce them.
///
/// Shared by every transport, so the stored result cannot depend on how the bytes
/// were obtained: `av index` and the web interface both land here, and both pass
/// `filename: None` so the description derives from the location rather than from
/// whatever a fetcher called the file. That matters because the browser SDK
/// invents names like `public-file-<8 hex>.bin`, which would otherwise become the
/// stored filename and the basis of the embedded description.
///
/// Deliberately has no reachability check, so it must only ever be reached with
/// bytes resolved from the address the caller supplied.
pub fn index_resource_from_bytes(
    state: &StartupState,
    conn: &Connection,
    provider: &MiniLmProvider,
    bytes: &[u8],
    uri: &str,
    tags: &[String],
    force: bool,
) -> CliResult<IndexOutcome> {
    let bytes_len = bytes.len();

    // --- Step 3: Ingest and build ResourceDescriptor ------------------------
    let mut resource =
        ingest_bytes(bytes, None, uri).map_err(|e| CliError::Ingest(e.to_string()))?;

    if !tags.is_empty() {
        av_ingest::apply_tags(&mut resource, tags);
    }

    // --- Step 3b: Refuse a locally blocked content hash -----------------------
    // Checked before any store write, which also means before the Step 7
    // announce: `av index` is a third route by which a resource can leave this
    // node, alongside serving query responses and reinforcing a search hit, and
    // the search-layer filter does not cover it.
    //
    // Fails *closed*: if the block list cannot be read, indexing stops. Treating
    // a read error as "not blocked" would let the one resource the user
    // explicitly suppressed leave the node, which is the exact outcome a block
    // exists to prevent.
    if av_store::repo::blocks::is_blocked(
        conn,
        av_store::repo::blocks::BlockKind::Resource,
        &resource.id,
    )
    .map_err(|e| CliError::Database(format!("could not read the block list: {e}")))?
    {
        return Err(CliError::Validation(format!(
            "resource {} is blocked; run 'av unblock --resource {}' to index it again",
            resource.id, resource.id
        )));
    }

    // --- Step 4: Check for duplicate ----------------------------------------
    // Primary: dedup by location (same URL)
    let existing_by_loc = av_store::repo::resources::get_by_location(conn, uri)
        .map_err(|e| CliError::Database(e.to_string()))?;

    if let Some(existing) = existing_by_loc {
        if !force {
            return Ok(IndexOutcome {
                resource_id: existing.id,
                location: uri.to_string(),
                mime_type: resource.mime_type,
                kind: format!("{:?}", resource.kind),
                description: resource.description_text,
                embedding_dim: 0,
                bytes_len,
                duplicate: true,
                duplicate_by: Some("location"),
                broadcast: false,
            });
        }

        // --force: delete existing record (cascade removes embeddings + feedback)
        av_store::repo::resources::delete(conn, &existing.id)
            .map_err(|e| CliError::Database(e.to_string()))?;
    }

    // Secondary: dedup by content hash (different URL, same content)
    let existing_by_hash = av_store::repo::resources::get(conn, &resource.id)
        .map_err(|e| CliError::Database(e.to_string()))?;

    if existing_by_hash.is_some() && !force {
        return Ok(IndexOutcome {
            resource_id: resource.id,
            location: uri.to_string(),
            mime_type: resource.mime_type,
            kind: format!("{:?}", resource.kind),
            description: resource.description_text,
            embedding_dim: 0,
            bytes_len,
            duplicate: true,
            duplicate_by: Some("content"),
            broadcast: false,
        });
    }

    // --- Step 5: Embed -------------------------------------------------------
    let vector = provider
        .embed_text(&resource.description_text)
        .map_err(|e| CliError::Model(e.to_string()))?;

    let pid = profile_id(provider.profile());
    let l2_norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();

    let embedding = EmbeddingRecord {
        resource_id: resource.id.clone(),
        profile_id: pid.clone(),
        vector,
        l2_norm,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    };
    let embedding_dim = embedding.vector.len();

    // --- Step 6: Store locally -----------------------------------------------
    av_store::repo::resources::insert(conn, &resource)
        .map_err(|e| CliError::Database(e.to_string()))?;
    av_store::repo::embeddings::insert_profile(conn, &pid, provider.profile())
        .map_err(|e| CliError::Database(e.to_string()))?;
    av_store::repo::embeddings::insert(conn, &embedding)
        .map_err(|e| CliError::Database(e.to_string()))?;

    // --- Step 7: Broadcast via network if x0x is running --------------------
    let broadcast = if let Some(ref x0x_cfg) = state.x0x_config {
        let net_client = Arc::new(X0xNetClient::new(x0x_cfg.clone()));
        let dispatcher = MessageDispatcher::new(net_client);

        let result_payload = ResourceResult {
            resource_id: resource.id.clone(),
            location: resource.location.clone(),
            location_scheme: resource.location_scheme.clone(),
            description_text: resource.description_text.clone(),
            mime_type: resource.mime_type.clone(),
            score: 1.0, // freshly indexed, high confidence
        };

        // Publish as a response with no query_id — acts as an announcement
        match dispatcher.publish_response("av-index-announce", vec![result_payload]) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Failed to broadcast indexed resource: {}", e);
                false
            }
        }
    } else {
        false
    };

    Ok(IndexOutcome {
        resource_id: resource.id,
        location: uri.to_string(),
        mime_type: resource.mime_type,
        kind: format!("{:?}", resource.kind),
        description: resource.description_text,
        embedding_dim,
        bytes_len,
        duplicate: false,
        duplicate_by: None,
        broadcast,
    })
}

pub fn run(
    cli: crate::Cli,
    state: StartupState,
    uri: String,
    tags: Option<String>,
    no_download: bool,
    no_verify: bool,
    force: bool,
) -> CliResult<()> {
    let uri = av_ingest::location::normalize_uri(&uri);

    // A manifest link needs no daemon at all — the bytes are the link — so it
    // is refused before Step 0 could offer to install one for it.
    if av_ingest::location::is_manifest_link(&uri) {
        return Err(CliError::Validation(
            av_ingest::location::manifest_link_error(&uri),
        ));
    }

    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    // --- Step 0: Ensure antd or ant CLI is available for ant:// URIs ----
    let uri_lower = uri.to_lowercase();
    if (uri_lower.starts_with("ant://") || uri_lower.starts_with("autonomi://"))
        && !state.antd_running
        && !ant_cli_binary_available()
    {
        if cli.non_interactive {
            return Err(CliError::MissingDependency {
                code: "no_download_backend",
                detail: concat!(
                    "Neither antd daemon nor ant CLI are available. ",
                    "Install ant CLI: curl -fsSL https://raw.githubusercontent.com/",
                    "WithAutonomi/ant-client/main/install.sh | bash. ",
                    "Or install antd from: https://github.com/WithAutonomi/ant-sdk/releases"
                )
                .to_string(),
            });
        }

        println!("antd daemon is not running. ant CLI is not installed.");
        let antd_started = Confirm::new()
            .with_prompt("Would you like to try starting the antd daemon?")
            .default(true)
            .interact()
            .map_err(|e| CliError::Other(e.to_string()))?
            && start_antd_daemon();

        if antd_started {
            println!("antd daemon started successfully.");
        } else if Confirm::new()
            .with_prompt("Would you like to install the ant CLI?")
            .default(true)
            .interact()
            .map_err(|e| CliError::Other(e.to_string()))?
            && install_ant_cli()
        {
            // The PowerShell installer may put ant.exe in various locations
            // that aren't yet in PATH for the current terminal session.
            refresh_ant_path();
            println!("ant CLI installed successfully.");
        } else {
            println!();
            #[cfg(windows)]
            {
                println!("Install ant CLI:");
                println!(
                    "  irm https://raw.githubusercontent.com/\
                     WithAutonomi/ant-client/main/install.ps1 | iex"
                );
            }
            #[cfg(not(windows))]
            {
                println!("Install ant CLI:");
                println!(
                    "  curl -fsSL https://raw.githubusercontent.com/\
                     WithAutonomi/ant-client/main/install.sh | bash"
                );
            }
            println!();
            println!("Or download antd from:");
            println!("  https://github.com/WithAutonomi/ant-sdk/releases");
            println!();
            return Err(CliError::Daemon(
                "antd daemon or ant CLI is required for indexing ant:// URIs. Exiting.".to_string(),
            ));
        }
    }

    // Tags are interactive by default, except in machine mode.
    let effective_tags: Vec<String> = if let Some(ref t) = tags {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if !cli.non_interactive {
        let raw: String = Input::new()
            .with_prompt("Tags (comma-separated, or Enter to skip)")
            .allow_empty(true)
            .interact_text()
            .map_err(|e| CliError::Other(e.to_string()))?;
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![]
    };

    let provider = MiniLmProvider::new().map_err(|e| CliError::Model(e.to_string()))?;

    // Download progress goes to a spinner on a TTY, and nowhere in machine mode.
    let bar = (!no_download && !cli.non_interactive).then(|| {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("  {spinner:.cyan} {msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
        );
        bar.set_message("Downloading...");
        bar.enable_steady_tick(std::time::Duration::from_millis(80));
        bar
    });
    let finished = std::cell::Cell::new(false);
    let notify = |event: DownloadEvent| match event {
        DownloadEvent::Status(msg) => {
            if let Some(b) = &bar {
                b.set_message(msg.to_string());
            }
        }
        DownloadEvent::SubprocessOutput => {
            if let Some(b) = &bar {
                b.finish_and_clear();
                finished.set(true);
            }
        }
        // Unreachable from this path: it fetches with `DownloadOptions::default`,
        // which leaves `chunk_progress` off, so the ant CLI is never asked for a
        // diagnostics file. Matched rather than ignored so that turning the option
        // on here has to be a decision instead of an accident.
        DownloadEvent::Chunk(_) => {}
    };

    let outcome = index_resource(
        &state,
        &conn,
        &provider,
        &uri,
        &effective_tags,
        no_download,
        no_verify,
        force,
        Some(&notify),
    )?;

    // Clear the spinner if the download finished without a subprocess boundary.
    if let Some(b) = &bar
        && !finished.get()
    {
        b.finish_with_message(format!("Downloaded ({} KB)", outcome.bytes_len / 1024));
    }

    let output_json = json!({
        "ok": true,
        "resource_id": outcome.resource_id,
        "location": outcome.location,
        "mime_type": outcome.mime_type,
        "kind": outcome.kind,
        "description": outcome.description,
        "embedding_dim": outcome.embedding_dim,
        "duplicate": outcome.duplicate,
        "broadcast": outcome.broadcast,
    });

    print_output(
        cli.non_interactive,
        || {
            if outcome.duplicate {
                match outcome.duplicate_by {
                    Some("content") => println!(
                        "  {} Content already indexed at a different location (use --force to re-index)",
                        console::style("~").yellow()
                    ),
                    _ => println!(
                        "  {} Resource already indexed (use --force to re-index)",
                        console::style("~").yellow()
                    ),
                }
                println!("  resource_id: {}", abbrev(&outcome.resource_id, 16));
                return;
            }

            println!(
                "Indexing: {}",
                console::style(&outcome.location).cyan().bold()
            );
            println!(
                "  {} Detected: {}",
                console::style("✓").green(),
                outcome.mime_type
            );
            println!(
                "  {} Description: \"{}\"",
                console::style("✓").green(),
                outcome.description
            );
            println!(
                "  {} Embedded ({} dimensions)",
                console::style("✓").green(),
                outcome.embedding_dim
            );
            println!("  {} Stored locally", console::style("✓").green());
            if outcome.broadcast {
                println!("  {} Broadcast to network", console::style("✓").green());
            }
            println!("\n  resource_id: {}", outcome.resource_id);
        },
        &output_json,
    );

    Ok(())
}
