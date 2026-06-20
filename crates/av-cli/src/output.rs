use serde::Serialize;

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
        let err_type = match error {
            crate::cmd::CliError::Daemon(_) => "daemon_error",
            crate::cmd::CliError::Database(_) => "database_error",
            crate::cmd::CliError::Model(_) => "model_error",
            crate::cmd::CliError::Ingest(_) => "ingest_error",
            crate::cmd::CliError::Network(_) => "network_error",
            crate::cmd::CliError::Validation(_) => "validation_error",
            crate::cmd::CliError::Io(_) => "io_error",
            crate::cmd::CliError::Json(_) => "json_error",
            crate::cmd::CliError::Other(_) => "other_error",
        };

        let err_json = serde_json::json!({
            "ok": false,
            "error": err_type,
            "detail": error.to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&err_json).unwrap());
    } else {
        eprintln!(
            "{}",
            console::style(format!("Error: {}", error)).red().bold()
        );
    }
}
