use std::cell::RefCell;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use av_core::types::EmbeddingRecord;
use av_embed::minilm::MiniLmProvider;
use av_embed::provider::{profile_id, EmbeddingProvider};
use av_ingest::ingest::ingest_bytes;
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::ResourceResult;
use dialoguer::{Confirm, Input};
use indicatif::{ProgressBar, ProgressStyle};
use serde_json::json;

use crate::cmd::{CliError, CliResult};
use crate::download::{download_content, verify_uri_exists, DownloadEvent};
use crate::output::print_output;
use crate::startup::{ant_cli_binary_available, install_ant_cli, start_antd_daemon, StartupState};

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
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    // --- Step 0: Ensure antd or ant CLI is available for ant:// URIs ----
    let uri_lower = uri.to_lowercase();
    if uri_lower.starts_with("ant://") || uri_lower.starts_with("autonomi://") {
        if !state.antd_running {
            if ant_cli_binary_available() {
                // ant CLI is available — skip antd prompts, use CLI as download fallback
            } else if cli.non_interactive {
                let err = json!({
                    "ok": false,
                    "error": "no_download_backend",
                    "detail": concat!(
                        "Neither antd daemon nor ant CLI are available. ",
                        "Install ant CLI: curl -fsSL https://raw.githubusercontent.com/",
                        "WithAutonomi/ant-client/main/install.sh | bash. ",
                        "Or install antd from: https://github.com/WithAutonomi/ant-sdk/releases"
                    )
                });
                println!("{}", serde_json::to_string_pretty(&err).unwrap());
                std::process::exit(1);
            } else {
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
                    // that aren't yet in PATH for the current terminal session. Ensure
                    // common install directories are visible.
                    #[cfg(windows)]
                    {
                        let candidates = [
                            format!("{}\\AppData\\Local\\ant\\bin", std::env::var("USERPROFILE").unwrap_or_default()),
                            format!("{}\\.local\\bin", std::env::var("USERPROFILE").unwrap_or_default()),
                        ];
                        let current = std::env::var("PATH").unwrap_or_default();
                        let mut to_add = Vec::new();
                        for path in &candidates {
                            if !path.is_empty()
                                && std::path::Path::new(path).join("ant.exe").exists()
                                && !current.split(';').any(|p| p == path.as_str())
                            {
                                to_add.push(path.clone());
                            }
                        }
                        if !to_add.is_empty() {
                            let new_path = to_add.join(";") + ";" + &current;
                            // SAFETY: single-threaded at startup before any env reads race
                            unsafe { std::env::set_var("PATH", new_path); }
                        }
                    }
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
                        "antd daemon or ant CLI is required for indexing ant:// URIs. Exiting."
                            .to_string(),
                    ));
                }
            }
        }
    }

    // --- Step 1: Verify URI (unless skipped) --------------------------------
    if !no_verify {
        if let Err(e) = verify_uri_exists(&uri) {
            return Err(CliError::Network(format!("URI unreachable: {}", e)));
        }
    }

    // --- Step 2: Download content (unless skipped) --------------------------
    let bytes = if no_download {
        // Use the URI as a 1-byte placeholder so ingest can derive location info
        uri.as_bytes().to_vec()
    } else {
        let pb = if !cli.non_interactive {
            let bar = ProgressBar::new_spinner();
            bar.set_style(
                ProgressStyle::with_template("  {spinner:.cyan} {msg}")
                    .unwrap()
                    .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
            );
            bar.set_message("Downloading...");
            bar.enable_steady_tick(std::time::Duration::from_millis(80));
            Some(bar)
        } else {
            None
        };

        let pb_cell = RefCell::new(pb);

        let notify = |event: DownloadEvent<'_>| {
            match event {
                DownloadEvent::Status(msg) => {
                    if let Some(ref bar) = *pb_cell.borrow() {
                        bar.set_message(msg.to_string());
                    }
                }
                DownloadEvent::SubprocessOutput => {
                    if let Some(bar) = pb_cell.borrow_mut().take() {
                        bar.finish_and_clear();
                    }
                }
            }
        };
        let data = download_content(&uri, Some(&notify as &dyn Fn(DownloadEvent)))?;

        if let Some(bar) = pb_cell.into_inner() {
            bar.finish_with_message(format!("Downloaded ({} KB)", data.len() / 1024));
        }
        data
    };

    // --- Step 3: Ingest and build ResourceDescriptor ------------------------
    let mut resource =
        ingest_bytes(&bytes, None, &uri).map_err(|e| CliError::Ingest(e.to_string()))?;

    // Append tags to description if provided
    let effective_tags: Vec<String> = if let Some(ref t) = tags {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if !cli.non_interactive {
        // Interactive: prompt for tags
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

    if !effective_tags.is_empty() {
        let tag_str = effective_tags.join(", ");
        resource.description_text = format!("{} tagged as: {}", resource.description_text, tag_str);
    }

    // --- Step 4: Check for duplicate ----------------------------------------
    // Primary: dedup by location (same URL)
    let existing_by_loc = av_store::repo::resources::get_by_location(&conn, &uri)
        .map_err(|e| CliError::Database(e.to_string()))?;

    if let Some(existing) = existing_by_loc {
        if !force {
            let output_json = json!({
                "ok": true,
                "resource_id": existing.id,
                "location": uri,
                "mime_type": resource.mime_type,
                "kind": format!("{:?}", resource.kind),
                "duplicate": true,
                "skipped": true,
            });

            print_output(
                cli.non_interactive,
                || {
                    println!(
                        "  {} Resource already indexed (use --force to re-index)",
                        console::style("~").yellow()
                    );
                    println!("  resource_id: {}", &existing.id[..16]);
                },
                &output_json,
            );
            return Ok(());
        }

        // --force: delete existing record (cascade removes embeddings + feedback)
        av_store::repo::resources::delete(&conn, &existing.id)
            .map_err(|e| CliError::Database(e.to_string()))?;
    }

    // Secondary: dedup by content hash (different URL, same content)
    let existing_by_hash = av_store::repo::resources::get(&conn, &resource.id)
        .map_err(|e| CliError::Database(e.to_string()))?;

    if existing_by_hash.is_some() && !force {
        let output_json = json!({
            "ok": true,
            "resource_id": resource.id,
            "location": uri,
            "mime_type": resource.mime_type,
            "kind": format!("{:?}", resource.kind),
            "duplicate": true,
            "skipped": true,
        });

        print_output(
            cli.non_interactive,
            || {
                println!(
                    "  {} Content already indexed at a different location (use --force to re-index)",
                    console::style("~").yellow()
                );
                println!("  resource_id: {}", &resource.id[..16]);
            },
            &output_json,
        );
        return Ok(());
    }

    // --- Step 5: Embed -------------------------------------------------------
    let provider = MiniLmProvider::new().map_err(|e| CliError::Model(e.to_string()))?;

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

    // --- Step 6: Store locally -----------------------------------------------
    av_store::repo::resources::insert(&conn, &resource)
        .map_err(|e| CliError::Database(e.to_string()))?;
    av_store::repo::embeddings::insert_profile(&conn, &pid, provider.profile())
        .map_err(|e| CliError::Database(e.to_string()))?;
    av_store::repo::embeddings::insert(&conn, &embedding)
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

    // --- Output --------------------------------------------------------------
    let output_json = json!({
        "ok": true,
        "resource_id": resource.id,
        "location": uri,
        "mime_type": resource.mime_type,
        "kind": format!("{:?}", resource.kind),
        "description": resource.description_text,
        "embedding_dim": embedding.vector.len(),
        "duplicate": false,
        "broadcast": broadcast,
    });

    print_output(
        cli.non_interactive,
        || {
            println!("Indexing: {}", console::style(&uri).cyan().bold());
            println!(
                "  {} Detected: {}",
                console::style("✓").green(),
                resource.mime_type
            );
            println!(
                "  {} Description: \"{}\"",
                console::style("✓").green(),
                resource.description_text
            );
            println!(
                "  {} Embedded ({} dimensions)",
                console::style("✓").green(),
                embedding.vector.len()
            );
            println!("  {} Stored locally", console::style("✓").green());
            if broadcast {
                println!("  {} Broadcast to network", console::style("✓").green());
            }
            println!("\n  resource_id: {}", resource.id);
        },
        &output_json,
    );

    Ok(())
}
