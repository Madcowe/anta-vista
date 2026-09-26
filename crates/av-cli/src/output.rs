use serde::Serialize;

/// Truncate `s` to at most `n` bytes, without panicking on short input.
///
/// Agent ids, resource ids and peer addresses all come off the network and can
/// be shorter than any fixed prefix width, so never index them directly.
pub fn abbrev(s: &str, n: usize) -> &str {
    &s[..s.len().min(n)]
}

pub fn print_output<T: Serialize>(
    non_interactive: bool,
    interactive_render: impl FnOnce(),
    json_value: &T,
) {
    if non_interactive {
        match serde_json::to_string_pretty(json_value) {
            Ok(s) => println!("{}", s),
            Err(e) => eprintln!("Failed to serialize output: {}", e),
        }
    } else {
        interactive_render();
    }
}

pub fn print_error(non_interactive: bool, error: &crate::cmd::CliError) {
    if non_interactive {
        // These two carry their own machine-readable code and an already
        // human-readable detail, so they bypass the type-name mapping and
        // `to_string()` prefixing below.
        let err_json = match error {
            crate::cmd::CliError::MissingDependency { code, detail } => {
                serde_json::json!({ "ok": false, "error": code, "detail": detail })
            }
            crate::cmd::CliError::NoSubcommand { detail, hint } => {
                serde_json::json!({ "ok": false, "error": "no_subcommand", "detail": detail, "hint": hint })
            }
            other => {
                let err_type = match other {
                    crate::cmd::CliError::Daemon(_) => "daemon_error",
                    crate::cmd::CliError::Database(_) => "database_error",
                    crate::cmd::CliError::Model(_) => "model_error",
                    crate::cmd::CliError::Ingest(_) => "ingest_error",
                    crate::cmd::CliError::Network(_) => "network_error",
                    crate::cmd::CliError::Validation(_) => "validation_error",
                    crate::cmd::CliError::Io(_) => "io_error",
                    crate::cmd::CliError::Json(_) => "json_error",
                    crate::cmd::CliError::Other(_) => "other_error",
                    crate::cmd::CliError::MissingDependency { .. } => unreachable!(),
                    crate::cmd::CliError::NoSubcommand { .. } => unreachable!(),
                };

                serde_json::json!({
                    "ok": false,
                    "error": err_type,
                    "detail": other.to_string(),
                })
            }
        };
        println!("{}", serde_json::to_string_pretty(&err_json).unwrap());
    } else {
        eprintln!(
            "{}",
            console::style(format!("Error: {}", error)).red().bold()
        );
    }
}
