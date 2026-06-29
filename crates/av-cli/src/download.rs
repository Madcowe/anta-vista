use crate::cmd::{CliError, CliResult};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use tempfile::NamedTempFile;
use ureq;

pub enum DownloadEvent<'a> {
    Status(&'a str),
    SubprocessOutput,
}

pub fn download_content(uri: &str, on_event: Option<&dyn Fn(DownloadEvent)>) -> CliResult<Vec<u8>> {
    let location_info = av_ingest::location::analyze_location(uri);
    let scheme = location_info.scheme.as_deref().unwrap_or("file");

    match scheme {
        "http" | "https" => {
            if let Some(f) = on_event {
                f(DownloadEvent::Status("Downloading from HTTP..."));
            }
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
            if let Some(f) = on_event {
                f(DownloadEvent::Status("Checking antd data endpoint..."));
            }
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
                                if let Some(f) = on_event {
                                    f(DownloadEvent::Status("Processing small data..."));
                                }
                                return Ok(decoded);
                            }
                        }
                    }
                }
                Err(_) => {} // Fallback to file download if GET fails
            }

            // Fallback: Stream via public data streaming endpoint
            if let Some(f) = on_event {
                f(DownloadEvent::Status("Downloading from Autonomi network..."));
            }
            let stream_url = format!("http://localhost:8082/v1/data/public/{}/stream", addr);
            let resp = match ureq::get(&stream_url).call() {
                Ok(r) => r,
                Err(antd_err) => {
                    let detail = match &antd_err {
                        ureq::Error::Status(code, _) => {
                            format!("antd returned status code {}", code)
                        }
                        ureq::Error::Transport(t)
                            if t.kind() == ureq::ErrorKind::ConnectionFailed =>
                        {
                            "antd daemon is not running".to_string()
                        }
                        _ => format!("antd daemon error: {}", antd_err),
                    };
                    if let Some(f) = on_event {
                        f(DownloadEvent::Status(&format!("{}, falling back to ant CLI...", detail)));
                    }
                    if let Some(f) = on_event {
                        f(DownloadEvent::SubprocessOutput);
                    }
                    return download_via_ant_cli(addr, on_event).map_err(|cli_err| {
                        CliError::Daemon(format!(
                            "antd REST API failed ({}); ant CLI also failed: {}",
                            detail, cli_err
                        ))
                    });
                }
            };

            if let Some(f) = on_event {
                f(DownloadEvent::Status("Downloading..."));
            }
            let mut bytes = Vec::new();
            resp.into_reader()
                .read_to_end(&mut bytes)
                .map_err(|e| CliError::Io(e))?;
            Ok(bytes)
        }
        "file" => {
            if let Some(f) = on_event {
                f(DownloadEvent::Status("Reading local file..."));
            }
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
            // Verify antd daemon is running via health endpoint.
            // Resource existence is checked during the actual download.
            let health_url = "http://localhost:8082/health";
            let resp = ureq::get(health_url).call();
            match resp {
                Ok(resp) if resp.status() == 200 => Ok(()),
                Ok(_) => Err(CliError::Daemon(
                    "antd daemon returned unhealthy status".to_string(),
                )),
                Err(ureq::Error::Transport(e)) if e.kind() == ureq::ErrorKind::ConnectionFailed => {
                    Err(CliError::Daemon(
                        "antd daemon is not running. Start it with 'antd start'."
                            .to_string(),
                    ))
                }
                Err(e) => Err(CliError::Daemon(format!(
                    "antd daemon unreachable: {}",
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

fn download_via_ant_cli(addr: &str, on_event: Option<&dyn Fn(DownloadEvent)>) -> CliResult<Vec<u8>> {
    let tmp = NamedTempFile::new().map_err(CliError::Io)?;
    let tmp_path = tmp.path().to_owned();

    if let Some(f) = on_event {
        f(DownloadEvent::Status("Downloading via ant CLI..."));
    }
    if let Some(f) = on_event {
        f(DownloadEvent::SubprocessOutput);
    }

    let status = Command::new("ant")
        .args(["file", "download", "-o"])
        .arg(&tmp_path)
        .arg(addr)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CliError::Daemon(
                    "ant CLI not found. Install it or ensure antd daemon is running.".to_string(),
                )
            } else {
                CliError::Io(e)
            }
        })?;

    if !status.success() {
        return Err(CliError::Daemon(format!(
            "ant file download failed (exit: {:?})",
            status.code()
        )));
    }

    fs::read(&tmp_path).map_err(CliError::Io)
}
