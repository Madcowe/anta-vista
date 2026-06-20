use crate::cmd::{CliError, CliResult};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::fs;
use std::path::Path;
use tempfile::NamedTempFile;
use ureq;

pub fn download_content(uri: &str) -> CliResult<Vec<u8>> {
    let location_info = av_ingest::location::analyze_location(uri);
    let scheme = location_info.scheme.as_deref().unwrap_or("file");

    match scheme {
        "http" | "https" => {
            let resp = ureq::get(uri)
                .call()
                .map_err(|e| CliError::Network(format!("Failed to download {}: {}", uri, e)))?;

            let mut bytes = Vec::new();
            std::io::copy(&mut resp.into_reader(), &mut bytes).map_err(|e| CliError::Io(e))?;
            Ok(bytes)
        }
        "ant" => {
            // Address is the 64-hex canonical part of the URI.
            // e.g. ant://<64-hex-address>
            let canonical = location_info
                .canonical
                .as_deref()
                .ok_or_else(|| CliError::Validation(format!("Invalid ant:// URI: {}", uri)))?;

            let addr = canonical
                .strip_prefix("ant://")
                .ok_or_else(|| CliError::Validation(format!("Invalid ant:// URI: {}", uri)))?;

            // Try public data GET first (for single chunk / small data)
            let get_data_url = format!("http://localhost:8082/v1/data/public/{}", addr);
            match ureq::get(&get_data_url).call() {
                Ok(resp) => {
                    if let Ok(json) = resp.into_json::<serde_json::Value>() {
                        if let Some(b64_data) = json["data"].as_str() {
                            if let Ok(decoded) = BASE64.decode(b64_data) {
                                return Ok(decoded);
                            }
                        }
                    }
                }
                Err(_) => {} // Fallback to file download if GET fails
            }

            // Fallback: Download via public file POST endpoint (for multi-chunk files)
            let temp_file = NamedTempFile::new().map_err(|e| CliError::Io(e))?;
            let temp_path = temp_file.path().to_string_lossy().to_string();

            let payload = serde_json::json!({
                "address": addr,
                "dest_path": temp_path,
            });

            let resp = ureq::post("http://localhost:8082/v1/files/public/get").send_json(payload);

            match resp {
                Ok(_) => {
                    let bytes = fs::read(&temp_path).map_err(|e| CliError::Io(e))?;
                    Ok(bytes)
                }
                Err(e) => Err(CliError::Daemon(format!(
                    "Failed to download ant:// resource from local antd: {}. Is antd running?",
                    e
                ))),
            }
        }
        "file" => {
            // Local file. Strip file:// if present.
            let path_str = uri.strip_prefix("file://").unwrap_or(uri);
            let path = Path::new(path_str);
            let bytes = fs::read(path).map_err(|e| CliError::Io(e))?;
            Ok(bytes)
        }
        _ => Err(CliError::Validation(format!(
            "Unsupported URI scheme: {}",
            scheme
        ))),
    }
}

pub fn verify_uri_exists(uri: &str) -> CliResult<()> {
    let location_info = av_ingest::location::analyze_location(uri);
    let scheme = location_info.scheme.as_deref().unwrap_or("file");

    match scheme {
        "http" | "https" => {
            let resp = ureq::head(uri)
                .call()
                .map_err(|e| CliError::Network(format!("URI unreachable: {}", e)))?;
            if resp.status() >= 400 {
                return Err(CliError::Network(format!(
                    "URI returned status {}",
                    resp.status()
                )));
            }
            Ok(())
        }
        "ant" => {
            let canonical = location_info
                .canonical
                .as_deref()
                .ok_or_else(|| CliError::Validation(format!("Invalid ant:// URI: {}", uri)))?;
            let addr = canonical.strip_prefix("ant://").unwrap();

            // Check health of local antd daemon first
            let get_data_url = format!("http://localhost:8082/v1/data/public/{}", addr);
            let resp = ureq::head(&get_data_url).call();
            match resp {
                Ok(_) => Ok(()),
                Err(ureq::Error::Status(404, _)) => Err(CliError::Network(
                    "ant:// resource not found on network".to_string(),
                )),
                Err(e) => Err(CliError::Daemon(format!(
                    "antd daemon error or unreachable: {}",
                    e
                ))),
            }
        }
        "file" => {
            let path_str = uri.strip_prefix("file://").unwrap_or(uri);
            if Path::new(path_str).exists() {
                Ok(())
            } else {
                Err(CliError::Validation(format!(
                    "File does not exist: {}",
                    path_str
                )))
            }
        }
        _ => Err(CliError::Validation(format!(
            "Unsupported URI scheme: {}",
            scheme
        ))),
    }
}
