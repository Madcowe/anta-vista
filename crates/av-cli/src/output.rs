use serde::Serialize;

/// Truncate `s` to at most `n` bytes, without panicking on short input.
///
/// Agent ids, resource ids and peer addresses all come off the network and can
/// be shorter than any fixed prefix width, so never index them directly.
///
/// Byte-slicing a `str` at an arbitrary index panics when the index is not a
/// UTF-8 character boundary, and these values are not always ASCII — a name
/// record's original name is user text. So the cut is walked back to the nearest
/// boundary. (Ids that are meant to be truncated for display are hex or UUIDs
/// and so never hit this, but the helper must not be a panic waiting for the
/// first non-ASCII caller.)
pub fn abbrev(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut end = n;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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

#[cfg(test)]
mod tests {
    use super::abbrev;

    #[test]
    fn abbrev_leaves_short_input_alone() {
        assert_eq!(abbrev("abc", 16), "abc");
        assert_eq!(abbrev("", 16), "");
    }

    #[test]
    fn abbrev_cuts_at_the_requested_width() {
        assert_eq!(abbrev("0123456789abcdef", 16), "0123456789abcdef");
        assert_eq!(abbrev("0123456789abcdef0123", 16), "0123456789abcdef");
    }

    #[test]
    fn abbrev_does_not_panic_on_multibyte_input() {
        // A name record's original name is user text, so `abbrev` can be handed
        // non-ASCII. Byte-slicing a `str` at an arbitrary index panics when the
        // index is not a character boundary; this used to be a live panic.
        let s = "naïve-café-ünicode";
        for n in 0..=s.len() + 4 {
            let out = abbrev(s, n);
            assert!(s.starts_with(out), "abbrev({n}) produced {out:?}");
        }
    }

    #[test]
    fn abbrev_is_lossless_for_ascii_at_full_width() {
        let id = "aaaabbbbccccddddeeeeffff00001111";
        assert_eq!(abbrev(id, 64), id);
    }
}
