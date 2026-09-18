use crate::cmd::{CliResult};
use crate::output::print_output;
use serde_json::json;

/// Stop every running `av listen` background process.
///
/// Sweeps all listener processes (the PID-file-tracked one plus any other
/// running `av listen`), so after a binary upgrade you can be sure no stale
/// responder is still active before starting the new one.
pub fn run(cli: &crate::Cli, force: bool) -> CliResult<()> {
    let stopped = crate::listener::stop_all(force);

    let report = json!({
        "ok": true,
        "stopped": stopped,
        "count": stopped.len(),
        "force": force,
    });

    print_output(
        cli.non_interactive,
        || {
            if stopped.is_empty() {
                println!("No av listen processes were running.");
            } else {
                println!(
                    "Stopped {} av listen process{}: {}",
                    stopped.len(),
                    if stopped.len() == 1 { "" } else { "es" },
                    stopped
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if force {
                println!("Used force-kill.");
            }
        },
        &report,
    );

    Ok(())
}