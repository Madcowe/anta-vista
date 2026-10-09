use crate::cmd::{CliError, CliResult};
use crate::startup::ant_cli_binary_available;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::NamedTempFile;
use ureq;

/// One chunk of a record, retrieved.
///
/// Emitted once per **distinct** chunk, when a fetch of it finally succeeds, so
/// a consumer can sum them into a running count without having to know about
/// retries. See [`ChunkProgress::retried`] for the retry that is still reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkProgress {
    /// The chunk's index in the record, as the fetcher numbered it.
    pub index: u64,
    /// Bytes this chunk contributed.
    pub bytes: u64,
    /// Distinct chunks retrieved so far, this one included.
    pub chunks: u64,
    /// Bytes retrieved across distinct chunks so far.
    ///
    /// Deliberately *not* presented anywhere as a fraction of the file size: a
    /// record is self-encrypted, so the sum of its chunks does not equal its
    /// length. Measured on a 138,931-byte JPEG, the three chunks totalled
    /// 138,768 bytes. A bar reading "99%" because of padding would be a number
    /// nobody can explain.
    pub total_bytes: u64,
    /// This chunk failed at least once before succeeding.
    ///
    /// Real and worth saying: a chunk that timed out against one peer and came
    /// back from another is the difference between "this is slow" and "this is
    /// broken", and a reader watching only a byte count cannot tell them apart.
    pub retried: bool,
}

pub enum DownloadEvent<'a> {
    Status(&'a str),
    SubprocessOutput,
    Chunk(ChunkProgress),
}

/// How much a caller wants to know while a fetch runs.
#[derive(Debug, Clone, Copy, Default)]
pub struct DownloadOptions {
    /// Stream per-chunk progress as [`DownloadEvent::Chunk`].
    ///
    /// Off by default so the CLI's behaviour is unchanged: it draws its own
    /// `indicatif` bar from `Status` lines and lets the ant CLI write straight
    /// through to the terminal. Only a caller with somewhere else to put the
    /// detail — the web interface, streaming into a second tab — asks for this.
    pub chunk_progress: bool,
}

pub fn download_content(
    uri: &str,
    on_event: Option<&dyn Fn(DownloadEvent)>,
    opts: DownloadOptions,
) -> CliResult<Vec<u8>> {
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
        "autonomi" => {
            // Address is the 64-hex canonical part of the URI.
            // e.g. autonomi://<64-hex-address>
            if let Some(f) = on_event {
                f(DownloadEvent::Status("Checking antd data endpoint..."));
            }
            let canonical = location_info
                .canonical
                .as_deref()
                .ok_or_else(|| CliError::Validation(format!("Invalid autonomi:// URI: {}", uri)))?;

            let addr = canonical
                .strip_prefix("autonomi://")
                .ok_or_else(|| CliError::Validation(format!("Invalid autonomi:// URI: {}", uri)))?;

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
                f(DownloadEvent::Status(
                    "Downloading from Autonomi network...",
                ));
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
                        f(DownloadEvent::Status(&format!(
                            "{}, falling back to ant CLI...",
                            detail
                        )));
                    }
                    if let Some(f) = on_event {
                        f(DownloadEvent::SubprocessOutput);
                    }
                    return download_via_ant_cli(addr, on_event, opts).map_err(|cli_err| {
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
        "autonomi" => {
            // Check if antd daemon is running via health endpoint.
            let health_url = "http://localhost:8082/health";
            let healthy = match ureq::get(health_url).call() {
                Ok(resp) => resp.status() == 200,
                _ => false,
            };
            if healthy {
                return Ok(());
            }
            // antd not running — accept ant CLI as fallback
            if ant_cli_binary_available() {
                return Ok(());
            }
            // Neither is available — give a clear error with install instructions
            let msg = concat!(
                "No download backend available. ",
                "Start antd ('antd --cors') or install ant CLI: ",
                "curl -fsSL https://raw.githubusercontent.com/",
                "WithAutonomi/ant-client/main/install.sh | bash. ",
                "See also: https://github.com/WithAutonomi/ant-sdk/releases"
            );
            Err(CliError::Daemon(msg.to_string()))
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

fn download_via_ant_cli(
    addr: &str,
    on_event: Option<&dyn Fn(DownloadEvent)>,
    opts: DownloadOptions,
) -> CliResult<Vec<u8>> {
    let tmp = NamedTempFile::new().map_err(CliError::Io)?;
    let tmp_path = tmp.path().to_owned();

    if let Some(f) = on_event {
        f(DownloadEvent::Status("Downloading via ant CLI..."));
    }

    // The ant CLI writes one JSONL record per chunk fetch attempt to this file,
    // flushing each as it happens rather than at the end — measured on a real
    // 138 KB record, records appeared at t=10s of a ~17s fetch. That is what
    // makes per-chunk progress possible on this transport at all, since the CLI
    // itself reports nothing until it finishes.
    let diagnostics = opts
        .chunk_progress
        .then(|| NamedTempFile::new().map_err(CliError::Io))
        .transpose()?;
    let diag_path = diagnostics.as_ref().map(|d| d.path().to_owned());

    let mut cmd = Command::new("ant");
    cmd.args(["file", "download", "-o"])
        .arg(&tmp_path)
        .arg(addr);
    if let Some(path) = &diag_path {
        cmd.arg("--download-diagnostics").arg(path);
    } else if let Some(f) = on_event {
        // Nothing to stream, so the CLI writes its own progress straight through
        // and our sink should stand down rather than draw over it.
        f(DownloadEvent::SubprocessOutput);
    }
    cmd.stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    // The child is *spawned* rather than waited on, so this thread stays free to
    // poll the diagnostics file and drain it into `on_event` as records appear.
    // A watcher thread would need `&dyn Fn` to be `Sync` to cross into it, which
    // it is not, and a channel would only move the problem to whoever had to
    // drain it while blocked on the child.
    let mut child = cmd.spawn().map_err(spawn_error)?;
    let sink = |event: DownloadEvent<'_>| {
        if let Some(f) = on_event {
            f(event);
        }
    };

    let mut tailer = diag_path.map(DiagnosticsTailer::new);
    let status = loop {
        if let Some(tailer) = tailer.as_mut() {
            tailer.poll(&sink);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(DIAGNOSTICS_POLL),
            Err(e) => return Err(CliError::Io(e)),
        }
    };
    // One last pass: the final chunks are written just before the process exits,
    // and this is the only chance to see them.
    if let Some(tailer) = tailer.as_mut() {
        tailer.poll(&sink);
    }

    if !status.success() {
        return Err(CliError::Daemon(format!(
            "ant file download failed (exit: {:?})",
            status.code()
        )));
    }

    fs::read(&tmp_path).map_err(CliError::Io)
}

fn spawn_error(e: std::io::Error) -> CliError {
    if e.kind() == std::io::ErrorKind::NotFound {
        CliError::Daemon(
            "ant CLI not found. Install it or ensure antd daemon is running.".to_string(),
        )
    } else {
        CliError::Io(e)
    }
}

/// How often the diagnostics file is read while the ant CLI runs.
///
/// Also how long a finished child goes unnoticed, so it is bounded by the wait
/// the user sees rather than by the progress it reports. The CLI's own chunk
/// events are seconds apart, so this is not the thing being measured.
const DIAGNOSTICS_POLL: Duration = Duration::from_millis(150);

/// Follows the ant CLI's diagnostics file, absorbing each JSONL record as it is
/// flushed and turning it into progress.
///
/// Reads the file rather than the child's output because nothing the CLI writes
/// to stdout or stderr is machine-readable until the run ends: `--json` prints a
/// single summary object, and only on success.
struct DiagnosticsTailer {
    path: PathBuf,
    offset: u64,
    /// Bytes past the last newline, which may be half a record.
    pending: String,
    /// `chunk_index -> bytes`, so a retried chunk is counted once.
    done: HashMap<u64, u64>,
    /// Attempts that did not succeed, which is what turns a later success into a
    /// reported retry rather than an ordinary chunk.
    failed: HashMap<u64, ()>,
}

impl DiagnosticsTailer {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            pending: String::new(),
            done: HashMap::new(),
            failed: HashMap::new(),
        }
    }

    /// Take in whatever has appeared since the last call, emitting an event per
    /// newly completed chunk. Cheap and idempotent when there is nothing new.
    fn poll(&mut self, sink: &dyn Fn(DownloadEvent)) {
        match read_appended(&self.path, self.offset) {
            Ok((read_to, bytes)) => {
                self.offset = read_to;
                self.pending.push_str(&String::from_utf8_lossy(&bytes));
                // Only whole lines: a half-written record must wait for the next
                // poll rather than be parsed as a truncated one.
                while let Some(nl) = self.pending.find('\n') {
                    let line: String = self.pending.drain(..=nl).collect();
                    if let Some(ev) = chunk_event(line.trim(), &mut self.done, &mut self.failed) {
                        sink(ev);
                    }
                }
            }
            // The file is created by the child, so it legitimately does not exist
            // on the first poll and is removed from under us if the process dies
            // first. Neither is worth ending the fetch over: the child's exit
            // status is the authority on whether it worked.
            Err(_) => {}
        }
    }
}

/// Read whatever has been appended to `path` since `offset`, and the offset to
/// resume from.
fn read_appended(path: &Path, offset: u64) -> std::io::Result<(u64, Vec<u8>)> {
    use std::io::Seek;

    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= offset {
        return Ok((offset, Vec::new()));
    }
    file.seek(std::io::SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    // What was actually read, not the file length: the child may have appended
    // more between the stat and the read, and claiming bytes never seen would
    // skip them.
    Ok((offset + buf.len() as u64, buf))
}

/// Turn one diagnostics record into a progress event, or `None` for a record
/// that is not one — a chunk that failed, a duplicate of one already reported,
/// or a line that is not JSON at all.
fn chunk_event(
    line: &str,
    done: &mut HashMap<u64, u64>,
    failed: &mut HashMap<u64, ()>,
) -> Option<DownloadEvent<'static>> {
    if line.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let index = value.get("chunk_index")?.as_u64()?;
    let outcome = value
        .get("outcome")
        .and_then(|o| o.as_str())
        .unwrap_or_default();
    let bytes = value.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0);

    if outcome != "found" {
        // Real, and worth remembering: a chunk that timed out against one peer
        // and came back from another is the difference between "this is slow"
        // and "this is broken". The record also carries `bytes: 0`, so counting
        // it would add nothing, but a `found` record for a chunk already in
        // `failed` still counts once.
        failed.insert(index, ());
        return None;
    }
    if done.contains_key(&index) {
        return None;
    }
    done.insert(index, bytes);
    let progress = ChunkProgress {
        index,
        bytes,
        chunks: done.len() as u64,
        total_bytes: done.values().sum(),
        retried: failed.remove(&index).is_some(),
    };
    Some(DownloadEvent::Chunk(progress))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One record as the ant CLI writes it. Only the fields we read are set, so a
    /// test that passes cannot be passing because of a field we happen to ignore
    /// today.
    fn record(index: u64, outcome: &str, bytes: u64) -> String {
        format!(
            r#"{{"schema_version":4,"chunk_index":{index},"outcome":"{outcome}","bytes":{bytes},"route":"unverified"}}"#
        )
    }

    /// The chunk events in a sequence, ignoring the status lines.
    fn chunks(events: &[DownloadEvent]) -> Vec<ChunkProgress> {
        events
            .iter()
            .filter_map(|e| match e {
                DownloadEvent::Chunk(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    }

    /// The `(chunk_index, outcome, bytes)` feed from a real run against a
    /// 138,931-byte JPEG, in the order the CLI wrote it: chunk 2 timed out
    /// against one peer and came back from another.
    ///
    /// The case a naive implementation gets wrong. Counting attempts gives four
    /// chunks for a three-chunk record; summing every `bytes` field double-counts
    /// the retry; and presenting the total against the file size implies a
    /// fraction that self-encryption padding makes meaningless.
    const REAL_FEED: &[(u64, &str, u64)] = &[
        (1, "found", 46107),
        (2, "timeout", 0),
        (3, "found", 46331),
        (2, "found", 46330),
    ];

    #[test]
    fn each_distinct_chunk_is_reported_once() {
        let mut done = HashMap::new();
        let mut failed = HashMap::new();
        let first = chunk_event(&record(1, "found", 100), &mut done, &mut failed);
        let second = chunk_event(&record(2, "found", 250), &mut done, &mut failed);

        let got: Vec<_> = [first, second]
            .into_iter()
            .flatten()
            .map(|e| match e {
                DownloadEvent::Chunk(c) => c,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].chunks, 1);
        assert_eq!(got[0].total_bytes, 100);
        assert_eq!(got[1].chunks, 2, "a running count of distinct chunks");
        assert_eq!(got[1].total_bytes, 350);
        assert!(got.iter().all(|c| !c.retried));
    }

    #[test]
    fn a_chunk_that_failed_then_succeeded_is_counted_once_and_flagged() {
        let mut done = HashMap::new();
        let mut failed = HashMap::new();

        // The timeout carries `bytes: 0` in the real feed, but the point is that
        // it must not be counted at all, whatever it claims.
        assert!(chunk_event(&record(2, "timeout", 0), &mut done, &mut failed).is_none());
        let got = chunk_event(&record(2, "found", 46330), &mut done, &mut failed);

        let c = match got.expect("the retry succeeds") {
            DownloadEvent::Chunk(c) => c,
            _ => unreachable!(),
        };
        assert_eq!(c.chunks, 1, "two attempts, one chunk");
        assert_eq!(
            c.total_bytes, 46330,
            "the timed-out attempt contributed nothing"
        );
        assert!(c.retried, "and the user is told it was retried");
    }

    #[test]
    fn a_repeated_success_for_one_chunk_does_not_double_count() {
        // Defence in depth against the retry case: even if a fetcher reported the
        // same chunk twice as found, the running totals must hold still.
        let mut done = HashMap::new();
        let mut failed = HashMap::new();
        chunk_event(&record(1, "found", 100), &mut done, &mut failed);
        assert!(chunk_event(&record(1, "found", 100), &mut done, &mut failed).is_none());
        assert_eq!(done[&1], 100);
        assert_eq!(done.len(), 1);
    }

    #[test]
    fn a_line_that_is_not_a_chunk_record_is_skipped() {
        // The feed is another program's output. A version bump, a stray warning
        // line or a truncated write must not end the fetch or invent progress.
        let mut done = HashMap::new();
        let mut failed = HashMap::new();
        for line in [
            "",
            "   ",
            "not json at all",
            r#"{"summary":"the whole run"}"#,
            r#"{"outcome":"found","bytes":10}"#,
            r#"{"chunk_index":3}"#,
            r#"{"chunk_index":"seven","outcome":"found"}"#,
        ] {
            assert!(
                chunk_event(line, &mut done, &mut failed).is_none(),
                "{line:?} should not become progress"
            );
        }
        assert!(done.is_empty());

        // And a record with no `outcome` is not treated as a success: the default
        // is empty, and only an explicit `found` counts.
        assert!(chunk_event(r#"{"chunk_index":4,"bytes":10}"#, &mut done, &mut failed).is_none());
        assert!(done.is_empty());
    }

    #[test]
    fn the_running_totals_match_a_real_three_chunk_record() {
        let mut done = HashMap::new();
        let mut failed = HashMap::new();
        let mut reported = Vec::new();
        for (index, outcome, bytes) in REAL_FEED {
            if let Some(DownloadEvent::Chunk(c)) =
                chunk_event(&record(*index, outcome, *bytes), &mut done, &mut failed)
            {
                reported.push(c);
            }
        }

        assert_eq!(done.len(), 3, "three chunks, not four attempts");
        assert_eq!(reported.len(), 3, "and three events, not four");
        // 138,768 of chunk bytes for a 138,931-byte file: the gap is
        // self-encryption overhead, which is why the total is never presented as a
        // fraction of the file size.
        assert_eq!(done.values().sum::<u64>(), 138_768);
        assert_eq!(reported.last().unwrap().chunks, 3);
        assert_eq!(reported.last().unwrap().total_bytes, 138_768);

        // Exactly one chunk is flagged, and it is the one that actually retried.
        let retried: Vec<u64> = reported
            .iter()
            .filter(|c| c.retried)
            .map(|c| c.index)
            .collect();
        assert_eq!(retried, vec![2]);
    }

    /// The end-to-end shape: a file that grows between polls, read through the
    /// same path the running fetch uses.
    #[test]
    fn the_tailer_picks_up_records_appended_while_it_runs() {
        let file = NamedTempFile::new().expect("temp");
        let mut tailer = DiagnosticsTailer::new(file.path().to_path_buf());
        let seen: std::cell::RefCell<Vec<ChunkProgress>> = std::cell::RefCell::new(Vec::new());
        let sink = |e: DownloadEvent| {
            if let DownloadEvent::Chunk(c) = e {
                seen.borrow_mut().push(c);
            }
        };

        // Nothing there yet: the child creates the file itself.
        tailer.poll(&sink);
        assert!(seen.borrow().is_empty());

        // A whole record, then a *half* of one. The half must wait: parsing it
        // would report a chunk that never finished arriving.
        let mut body = format!("{}\n{{\"chunk_index\":2", record(1, "found", 100));
        fs::write(file.path(), &body).expect("write");
        tailer.poll(&sink);
        assert_eq!(seen.borrow().len(), 1, "only the complete line");
        assert_eq!(seen.borrow()[0].index, 1);

        // A record only counts once its newline is there, exactly as the CLI
        // writes it.
        body.push_str(",\"outcome\":\"found\",\"bytes\":250}\n");
        fs::write(file.path(), &body).expect("append");
        tailer.poll(&sink);
        let got = seen.borrow().clone();
        assert_eq!(
            got.len(),
            2,
            "the completed line is picked up on the next poll"
        );
        assert_eq!(got[1].index, 2);
        assert_eq!(got[1].total_bytes, 350);

        // Polling again with nothing new must not repeat anything.
        tailer.poll(&sink);
        assert_eq!(seen.borrow().len(), 2);
    }

    #[test]
    fn reading_past_a_missing_or_shrinking_file_is_not_an_error() {
        // The child creates the file, and deletes it if it dies first. Neither may
        // take the fetch down: its exit status is the authority, not the file.
        let missing = PathBuf::from("/nonexistent/av-diagnostics-does-not-exist.jsonl");
        assert_eq!(
            read_appended(&missing, 0).expect_err("missing").kind(),
            std::io::ErrorKind::NotFound
        );

        let file = NamedTempFile::new().expect("temp");
        fs::write(file.path(), b"{}\n").expect("write");
        // An offset past the end — the file was replaced by a shorter one — reads
        // as "nothing new" rather than panicking or rewinding.
        assert_eq!(
            read_appended(file.path(), 999).expect("read"),
            (999, Vec::new())
        );
    }
}
