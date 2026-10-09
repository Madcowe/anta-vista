pub use av_store::repo::blocks::BlockKind;
use av_store::repo::blocks::{self, BlockEntry};

use crate::cmd::{CliError, CliResult};
use crate::output::print_output;
use crate::startup::StartupState;
use rusqlite::Connection;
use serde_json::json;

/// What blocking a subject actually did, so the CLI and the web UI can report
/// it honestly rather than implying everything succeeded.
#[derive(Debug, Clone)]
pub struct BlockOutcome {
    pub kind: BlockKind,
    pub subject_id: String,
    /// False when the subject was already blocked and this call changed nothing.
    pub blocked: bool,
    /// Whether a local row for a resource was deleted. False when the resource
    /// was not held locally, or when `keep_local` was requested.
    pub removed_local: bool,
    /// Whether local data was deliberately retained by `--keep-local`.
    pub kept_local: bool,
    /// How many dependent rows the local delete cascaded away. These are
    /// `feedback_events` and `relevance_judgments` as well as the embedding,
    /// so it is reported rather than assumed.
    pub cascaded: usize,
    /// Whether the operator asked for the x0x contact block to be applied.
    pub x0x_requested: bool,
    /// `None` when no x0x escalation was requested. `Some(Ok(()))` when it
    /// applied. `Some(Err(..))` when the local block landed but the transport
    /// block did not.
    pub x0x_applied: Option<Result<(), String>>,
}

/// What unblocking did.
#[derive(Debug, Clone)]
pub struct UnblockOutcome {
    pub kind: BlockKind,
    pub subject_id: String,
    /// False when the subject was not blocked, so callers can distinguish
    /// "unblocked" from "was never blocked".
    pub unblocked: bool,
    /// How many blocks were lifted, when clearing a whole kind or all of them.
    pub removed: usize,
    pub x0x_requested: bool,
    pub x0x_applied: Option<Result<(), String>>,
}

/// Reject a subject id that is not shaped like the thing it claims to be.
///
/// An agent id is a 64-character hex string, because that is what x0x mints.
/// A mistyped id would otherwise be stored as a block that silently never
/// matches anything, which reads as "the block is working" while doing nothing.
fn validate_subject(kind: BlockKind, subject_id: &str) -> CliResult<()> {
    let ok = match kind {
        BlockKind::Agent => {
            subject_id.len() == 64 && subject_id.bytes().all(|b| b.is_ascii_hexdigit())
        }
        // Resource ids are SHA-256 hex, and name record ids are UUIDs. Accept
        // any non-empty token for those and let a non-match simply never fire.
        BlockKind::Resource | BlockKind::Name => !subject_id.trim().is_empty(),
    };
    if ok {
        Ok(())
    } else {
        Err(CliError::Validation(format!(
            "'{subject_id}' is not a valid {} id{}",
            kind.as_str(),
            if kind == BlockKind::Agent {
                " (expected 64 hex characters, as minted by x0x)"
            } else {
                ""
            }
        )))
    }
}

/// Accept a name record id with or without the `name:` prefix that search
/// results carry, so an operator can paste a result's `resource_id` straight in.
fn normalize_subject(kind: BlockKind, subject_id: &str) -> String {
    let trimmed = subject_id.trim();
    match kind {
        BlockKind::Name => trimmed.strip_prefix("name:").unwrap_or(trimmed).to_string(),
        _ => trimmed.to_string(),
    }
}

/// Catch a truncated id before it becomes a block that silently never matches.
///
/// A block is keyed by exact match, so a shortened hash is worse than no block
/// at all: it is stored, listed, and enforced against nothing, so the operator
/// believes the result is gone. The obvious way to get one is pasting an
/// abbreviated id, which is why search and resolve now print full ids — but a
/// bad paste is still possible, and it should be caught here rather than
/// discovered weeks later.
///
/// Only an id that is a strict prefix of something actually held locally can be
/// resolved to a longer id, so that is the case treated as an error. Anything
/// else is left alone: a network result's resource is legitimately not held
/// locally, and refusing to block it would be a real limitation.
fn expand_known_prefix(
    conn: &Connection,
    kind: BlockKind,
    subject_id: &str,
) -> CliResult<Option<String>> {
    let (table, column) = match kind {
        BlockKind::Resource => ("resources", "id"),
        BlockKind::Name => ("name_records", "record_id"),
        // Nothing local is keyed by an agent id.
        BlockKind::Agent => return Ok(None),
    };
    // `LIKE` wildcards are escaped because an id containing `%` or `_` is
    // user-supplied and must not turn into a pattern.
    let pattern = format!(
        "{}%",
        subject_id
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    // Two distinct parameters: `?1` is the pattern and `?2` the exact id.
    // Sharing one placeholder would compare the id against the *pattern* and
    // so fail to exclude the subject's own row, making every id look like an
    // abbreviation of itself.
    let sql = format!(
        "SELECT {column} FROM {table} \
         WHERE {column} LIKE ?1 ESCAPE '\\' AND {column} != ?2 LIMIT 1"
    );
    let found: Option<String> = conn
        .query_row(&sql, rusqlite::params![pattern, subject_id], |r| r.get(0))
        .ok();
    Ok(found)
}

/// Count the rows a local resource delete will take with it.
///
/// `resources` has three `ON DELETE CASCADE` dependants. Counting them first
/// means the report can say what was actually lost instead of implying the
/// embedding was the only casualty — losing the feedback events matters to an
/// operator deciding whether `--keep-local` is the option they want.
fn dependent_row_count(conn: &Connection, resource_id: &str) -> usize {
    conn.query_row(
        "SELECT
            (SELECT COUNT(*) FROM embeddings          WHERE resource_id = ?1)
          + (SELECT COUNT(*) FROM feedback_events     WHERE resource_id = ?1)
          + (SELECT COUNT(*) FROM relevance_judgments WHERE resource_id = ?1)",
        rusqlite::params![resource_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
    .max(0) as usize
}

/// Apply the x0x contact block for an agent, if the operator asked for it.
///
/// The local row is the policy and always lands; this is an escalation that
/// additionally stops the daemon forwarding *anything* from that agent, on
/// every topic. It needs the daemon up, so a failure is reported rather than
/// raised — a working local block is not a failure.
fn escalate_to_x0x(
    state: &StartupState,
    agent_id: &str,
    level: Option<&str>,
) -> Option<Result<(), String>> {
    let cfg = state.x0x_config.as_ref()?;
    let client = av_net_x0x::client::X0xNetClient::new(cfg.clone());
    let outcome = match level {
        Some(l) => client.set_contact_trust(agent_id, l),
        None => client.remove_contact(agent_id),
    };
    Some(outcome.map_err(|e| e.to_string()))
}

/// Block one subject.
///
/// `keep_local` applies to resources: by default a locally-held copy is deleted
/// (reclaiming its embedding and, via `ON DELETE CASCADE`, its feedback events
/// and relevance judgments). The block row is what enforces the policy — the
/// delete is hygiene, and it means an unblock cannot silently resurrect a
/// resource that was only ever stored to be discarded.
// Eight parameters is a lot, but they are all one policy decision: what is
// blocked, why, whether the local copy survives, and whether to escalate to the
// transport. Bundling them into a struct used in exactly one place would just
// move the same fields somewhere less readable.
#[allow(clippy::too_many_arguments)]
pub fn block_subject(
    conn: &Connection,
    state: &StartupState,
    kind: BlockKind,
    subject_id: &str,
    reason: Option<&str>,
    keep_local: bool,
    x0x: bool,
    source: &str,
) -> CliResult<BlockOutcome> {
    let subject_id = normalize_subject(kind, subject_id);
    validate_subject(kind, &subject_id)?;

    if let Some(full) = expand_known_prefix(conn, kind, &subject_id)? {
        return Err(CliError::Validation(format!(
            "'{subject_id}' is an abbreviation of a subject you hold locally, not a full id. \
             Blocking the short form would never match anything, so nothing would be filtered. \
             Use the full id: av block --{} {full}",
            kind.as_str()
        )));
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let blocked = blocks::add(conn, kind, &subject_id, reason, now, source)
        .map_err(|e| CliError::Database(e.to_string()))?;

    // Before the delete, and unconditionally: the location is written down in
    // exactly one place, and that place is the `resources` row this branch is
    // about to drop. A fetch route is handed an `autonomi://` address and cannot
    // derive a content hash from it, so without this record the block is
    // invisible to every fetch guard and `av block` appears to do nothing at all
    // once the local copy is gone — which is the default.
    //
    // Unconditional for `--keep-local` too. That resource can be purged later, at
    // which point the join fallback goes with it, and a block that quietly stops
    // applying once the file is cleaned up is not a block.
    if kind == BlockKind::Resource {
        blocks::record_blocked_locations(conn, &subject_id)
            .map_err(|e| CliError::Database(e.to_string()))?;
    }

    // Only a resource can have a local row to remove. A name block leaves the
    // record in place and merely hidden, because deleting it would also drop
    // the record the operator may want to un-hide; an agent block has no local
    // row at all.
    let mut removed_local = false;
    let mut cascaded = 0usize;
    if kind == BlockKind::Resource && !keep_local {
        // Count the dependent rows the delete will take with it, so the report
        // states what was actually lost instead of implying only the embedding
        // went. `resources` has three `ON DELETE CASCADE` dependants:
        // `embeddings`, `feedback_events` and `relevance_judgments`. Name
        // records are not foreign keys but are dropped with the resource they
        // point at, so they are counted here too.
        cascaded = dependent_row_count(conn, &subject_id);
        let cascade = blocks::delete_resource_cascade_with_names(conn, &subject_id)
            .map_err(|e| CliError::Database(e.to_string()))?;
        removed_local = cascade.resource_removed;
        cascaded += cascade.names_removed;
    }

    let x0x_applied = if x0x && kind == BlockKind::Agent {
        escalate_to_x0x(state, &subject_id, Some("blocked"))
    } else {
        None
    };

    Ok(BlockOutcome {
        kind,
        subject_id,
        blocked,
        removed_local,
        kept_local: kind == BlockKind::Resource && keep_local,
        cascaded,
        x0x_requested: x0x && kind == BlockKind::Agent,
        x0x_applied,
    })
}

/// Lift the block on one subject, or on everything.
pub fn unblock_subject(
    conn: &Connection,
    state: &StartupState,
    kind: BlockKind,
    subject_id: Option<&str>,
    clear_all: bool,
    x0x: bool,
) -> CliResult<UnblockOutcome> {
    if let Some(id) = subject_id {
        let id = normalize_subject(kind, id);
        validate_subject(kind, &id)?;
        // A short id is refused here for the same reason it is refused when
        // blocking: it cannot match the full id we actually stored. But an
        // unblock has to be able to *remove* a bad short block — that row
        // exists precisely because an earlier version let it be created, and
        // refusing it here would strand the user with no way to clean it up
        // except `av unblock --all`. So the exact-match check comes first:
        // if a block row for this exact id exists, removing it is the goal.
        let already_blocked =
            blocks::is_blocked(conn, kind, &id).map_err(|e| CliError::Database(e.to_string()))?;
        if let (false, Some(full)) = (already_blocked, expand_known_prefix(conn, kind, &id)?) {
            return Err(CliError::Validation(format!(
                "'{id}' is an abbreviation of a subject you hold locally, and nothing is \
                 blocked under that exact id. Use the full id: av unblock --{} {full}",
                kind.as_str()
            )));
        }
        // Cleared before the block row is removed, and only for resources: the
        // recorded locations are keyed by this id and are the only thing a
        // location-based fetch guard can consult once the resource row is gone.
        // Leaving them behind would refuse fetches for a block the user has
        // explicitly lifted.
        if kind == BlockKind::Resource {
            blocks::forget_blocked_locations(conn, &id)
                .map_err(|e| CliError::Database(e.to_string()))?;
        }
        let removed =
            blocks::remove(conn, kind, &id).map_err(|e| CliError::Database(e.to_string()))?;
        let x0x_applied = if x0x && kind == BlockKind::Agent {
            escalate_to_x0x(state, &id, None)
        } else {
            None
        };
        return Ok(UnblockOutcome {
            kind,
            subject_id: id,
            unblocked: removed,
            removed: usize::from(removed),
            x0x_requested: x0x && kind == BlockKind::Agent,
            x0x_applied,
        });
    }

    // Every recorded location goes with `--all`, and with a `--kind` clear the
    // recorded locations of that kind. The table has no `subject_kind` of its own,
    // so this is filtered by joining the id against the block rows being cleared.
    if clear_all {
        blocks::forget_all_blocked_locations(conn)
            .map_err(|e| CliError::Database(e.to_string()))?;
    } else if kind == BlockKind::Resource {
        let recorded = blocks::resource_ids_with_blocked_locations(conn)
            .map_err(|e| CliError::Database(e.to_string()))?;
        for id in recorded {
            blocks::forget_blocked_locations(conn, &id)
                .map_err(|e| CliError::Database(e.to_string()))?;
        }
    }
    let removed = blocks::clear(conn, if clear_all { None } else { Some(kind) })
        .map_err(|e| CliError::Database(e.to_string()))?;
    Ok(UnblockOutcome {
        kind,
        subject_id: String::new(),
        unblocked: removed > 0,
        removed,
        // A bulk clear cannot enumerate x0x contacts to lift, and silently
        // leaving them blocked would make `av unblock --all` a lie.
        x0x_requested: false,
        x0x_applied: None,
    })
}

/// List every block, for `--list` and the web `/api/blocks`.
pub fn list_blocks(conn: &Connection) -> CliResult<Vec<BlockEntry>> {
    blocks::list_all(conn).map_err(|e| CliError::Database(e.to_string()))
}

/// Every block, plus the per-kind breakdown, read as one consistent snapshot.
///
/// Both surfaces that show blocks (`av block --list` and the web `/api/blocks`)
/// call this rather than each issuing its own queries, because two independent
/// reads of a table another process can write between them produce reports that
/// disagree with themselves: the count comes from the list while the breakdown
/// comes from a `GROUP BY` taken a moment later. Adding a block in that window
/// yields a summary whose parts do not add up to its total, which reads as a
/// bug in the block list rather than a race nobody should have to know about.
///
/// Deriving the breakdown from the rows already fetched means the invariant
/// holds by construction rather than by luck.
#[derive(Debug, Clone)]
pub struct BlockReport {
    /// Every blocked subject.
    pub blocks: Vec<BlockEntry>,
    /// How many subjects of each kind are blocked, as `(kind, count)`.
    ///
    /// Keyed on the kind's own label, so an unrecognised `subject_kind` is
    /// reported under itself instead of being folded into `resource`. Grouping
    /// happens on the raw stored string *before* parsing, for the opposite
    /// reason: parsing first would let two different stored kinds collapse onto
    /// one label and silently lose whichever row lost the merge.
    pub counts: Vec<(String, i64)>,
}

impl BlockReport {
    /// Total subjects blocked, which is `blocks.len()` by definition.
    pub fn count(&self) -> usize {
        self.blocks.len()
    }

    /// The JSON both surfaces return.
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "ok": true,
            "count": self.count(),
            "blocks": self
                .blocks
                .iter()
                .map(|e| json!({
                    "subject_kind": e.subject_kind_raw,
                    "subject_id": e.subject_id,
                    "reason": e.reason,
                    "blocked_at": e.blocked_at,
                    "source": e.source,
                }))
                .collect::<Vec<_>>(),
            "counts": self
                .counts
                .iter()
                .map(|(kind, n)| json!({ "subject_kind": kind, "count": n }))
                .collect::<Vec<_>>(),
        })
    }
}

/// Read every block and its per-kind breakdown in one pass over one snapshot.
///
/// Errors propagate. A `blocks` read that fails is not evidence that nothing is
/// blocked, so reporting must not quietly present a broken table as an empty
/// block list.
pub fn block_report(conn: &Connection) -> CliResult<BlockReport> {
    let blocks = list_blocks(conn)?;

    // First-seen order, over a list already ordered by `subject_kind`, so the
    // breakdown is stable rather than whatever order the rows happened to land
    // in. Grouped on the raw label so an unrecognised kind keeps its own
    // heading instead of being folded into `resource`, where a lookup would
    // find the wrong number.
    let mut counts: Vec<(String, i64)> = Vec::new();
    for entry in &blocks {
        let label = entry.subject_kind_raw.clone();
        match counts.iter_mut().find(|(kind, _)| *kind == label) {
            Some((_, n)) => *n += 1,
            None => counts.push((label, 1)),
        }
    }

    Ok(BlockReport { blocks, counts })
}

fn open_db() -> CliResult<Connection> {
    let path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    av_store::open(&path).map_err(|e| CliError::Database(e.to_string()))
}

/// Serialise a block outcome, including the honest x0x partial-success shape.
fn outcome_json(o: &BlockOutcome) -> serde_json::Value {
    json!({
        "ok": true,
        "subject_kind": o.kind.as_str(),
        "subject_id": o.subject_id,
        "blocked": o.blocked,
        "removed_local": o.removed_local,
        "kept_local": o.kept_local,
        "cascaded": o.cascaded,
        "x0x_requested": o.x0x_requested,
        "x0x_applied": match &o.x0x_applied {
            None => serde_json::Value::Null,
            Some(Ok(())) => json!(true),
            Some(Err(_)) => json!(false),
        },
        "x0x_error": o.x0x_applied.as_ref().and_then(|r| r.as_ref().err().cloned()),
        "warning": warning_for(o),
    })
}

pub fn warning_for(o: &BlockOutcome) -> Option<String> {
    if !o.blocked {
        return Some("This subject was already blocked; nothing changed.".into());
    }
    if o.cascaded > 0 {
        return Some(
            "Removed the local copy. That cascaded away its embedding, feedback events and \
             relevance judgments; use --keep-local to block without deleting."
                .to_string(),
        );
    }
    if o.x0x_requested && o.x0x_applied.is_none() {
        return Some(
            "Blocked locally. The x0x transport block was skipped because the daemon is not running."
                .into(),
        );
    }
    if let Some(Err(e)) = &o.x0x_applied {
        return Some(format!(
            "Blocked locally, but the x0x transport block failed: {e}. The peer can still \
             reach this node; only anta-vista results are filtered."
        ));
    }
    // A block whose id is shorter than a full SHA-256 and still hex is almost
    // certainly an abbreviation. It cannot match anything, so saying so is the
    // difference between a block that works and one that merely appears to.
    // `expand_known_prefix` already turned the locally-known case into a hard
    // error, so reaching here means the subject is not held locally — which is
    // normal for a network result and is why this is a warning, not a refusal.
    if looks_abbreviated(&o.subject_id) {
        return Some(format!(
            "'{}' looks like an abbreviated id (resource ids are 64 hex characters). A block \
             must match exactly, so if this is a shortened hash it will never match. Copy the \
             full id from the result's 'resource_id' line.",
            o.subject_id
        ));
    }
    None
}

/// Whether `id` is hex but shorter than a full SHA-256 hash, which is the
/// signature of a truncated id rather than a deliberately short one.
fn looks_abbreviated(id: &str) -> bool {
    !id.is_empty()
        && id.len() < 64
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && !id.chars().all(|c| c.is_ascii_digit())
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    cli: crate::Cli,
    state: StartupState,
    resource: Option<String>,
    agent: Option<String>,
    name: Option<String>,
    reason: Option<String>,
    keep_local: bool,
    x0x: bool,
    list: bool,
) -> CliResult<()> {
    let conn = open_db()?;

    if list {
        let report = block_report(&conn)?;
        let entries = &report.blocks;
        print_output(
            cli.non_interactive,
            || {
                if entries.is_empty() {
                    println!("No blocks. Nothing is filtered or refused.");
                    return;
                }
                for e in entries {
                    let reason = e
                        .reason
                        .as_deref()
                        .map(|r| format!(" — {}", console::style(r).dim()))
                        .unwrap_or_default();
                    // The full id, not an abbreviation: this is where an
                    // operator copies the id they need for `av unblock`, and a
                    // truncated one would fail to match.
                    println!(
                        "  {} {}{}",
                        console::style(&e.subject_kind_raw).yellow().bold(),
                        console::style(&e.subject_id).cyan(),
                        reason
                    );
                }
            },
            &report.to_json(),
        );
        return Ok(());
    }

    let selected: Vec<(BlockKind, String)> = [
        (BlockKind::Resource, resource),
        (BlockKind::Agent, agent),
        (BlockKind::Name, name),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|id| (k, id)))
    .collect();

    if selected.is_empty() {
        return Err(CliError::Validation(
            "Specify --resource, --agent or --name to block, or --list to show current blocks."
                .to_string(),
        ));
    }
    if selected.len() > 1 {
        return Err(CliError::Validation(
            "Block one subject at a time: pass a single --resource, --agent or --name. They are \
             independent policies, so blocking several at once hides which one you meant."
                .to_string(),
        ));
    }

    let (kind, subject_id) = selected.into_iter().next().expect("checked non-empty");
    let outcome = block_subject(
        &conn,
        &state,
        kind,
        &subject_id,
        reason.as_deref(),
        keep_local,
        x0x,
        "cli",
    )?;

    let report = outcome_json(&outcome);
    let kind_label = outcome.kind.as_str();
    let subject_label = outcome.subject_id.clone();
    let removed_local = outcome.removed_local;
    let kept_local = outcome.kept_local;
    let x0x_requested = outcome.x0x_requested;
    let x0x_failed = matches!(&outcome.x0x_applied, Some(Err(_)));
    let warning = warning_for(&outcome);

    print_output(
        cli.non_interactive,
        || {
            if !outcome.blocked {
                println!(
                    "{} {} {} was already blocked.",
                    console::style("~").yellow(),
                    kind_label,
                    console::style(&subject_label).cyan()
                );
                return;
            }
            println!(
                "{} Blocked {} {}",
                console::style("✓").green(),
                kind_label,
                console::style(&subject_label).cyan()
            );
            if removed_local {
                println!(
                    "  {} Removed the local copy (and its embedding, feedback and relevance rows)",
                    console::style("✓").green()
                );
            } else if kept_local {
                println!(
                    "  {} Kept the local copy; it stays hidden from search",
                    console::style("~").yellow()
                );
            }
            if x0x_requested && x0x_failed {
                println!(
                    "  {} x0x transport block FAILED — see the warning",
                    console::style("✗").red()
                );
            }
            match kind {
                BlockKind::Resource => println!(
                    "  {} It will not appear in results and will not be served to peers",
                    console::style("✓").green()
                ),
                BlockKind::Name => println!(
                    "  {} It will not resolve and will not be served to peers",
                    console::style("✓").green()
                ),
                BlockKind::Agent => println!(
                    "  {} Its answers are dropped and it is excluded from search fan-out",
                    console::style("✓").green()
                ),
            }
            if let Some(w) = warning {
                println!("  {}", console::style(w).yellow());
            }
        },
        &report,
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_unblock(
    cli: crate::Cli,
    state: StartupState,
    resource: Option<String>,
    agent: Option<String>,
    name: Option<String>,
    all: bool,
    x0x: bool,
    no_confirm: bool,
) -> CliResult<()> {
    let conn = open_db()?;

    if all {
        let total = block_report(&conn)?.count() as i64;
        if total == 0 {
            let report = json!({ "ok": true, "removed": 0, "unblocked": false });
            print_output(
                cli.non_interactive,
                || println!("Nothing is blocked."),
                &report,
            );
            return Ok(());
        }
        if !no_confirm && !cli.non_interactive {
            use dialoguer::Confirm;
            let ok = Confirm::new()
                .with_prompt(format!(
                    "Lift all {total} block(s)? Blocked resources stay deleted; they are not restored."
                ))
                .default(false)
                .interact()
                .map_err(|e| CliError::Other(e.to_string()))?;
            if !ok {
                return Err(CliError::Validation("Unblock aborted".to_string()));
            }
        }
    }

    let selected: Vec<(BlockKind, String)> = [
        (BlockKind::Resource, resource),
        (BlockKind::Agent, agent),
        (BlockKind::Name, name),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|id| (k, id)))
    .collect();

    if selected.is_empty() && !all {
        return Err(CliError::Validation(
            "Specify --resource, --agent or --name to unblock, or --all to lift every block."
                .to_string(),
        ));
    }
    if selected.len() > 1 {
        return Err(CliError::Validation(
            "Unblock one subject at a time: pass a single --resource, --agent or --name."
                .to_string(),
        ));
    }

    let outcome = if let Some((kind, subject_id)) = selected.into_iter().next() {
        unblock_subject(&conn, &state, kind, Some(&subject_id), false, x0x)?
    } else {
        unblock_subject(&conn, &state, BlockKind::Resource, None, true, x0x)?
    };

    let report = json!({
        "ok": true,
        "subject_kind": outcome.kind.as_str(),
        "subject_id": outcome.subject_id,
        "unblocked": outcome.unblocked,
        "removed": outcome.removed,
        "x0x_requested": outcome.x0x_requested,
        "x0x_applied": match &outcome.x0x_applied {
            None => serde_json::Value::Null,
            Some(Ok(())) => json!(true),
            Some(Err(_)) => json!(false),
        },
        "x0x_error": outcome.x0x_applied.as_ref().and_then(|r| r.as_ref().err().cloned()),
        "warning": if outcome.removed > 0 {
            Some("Lifted the block. A resource that was deleted when it was blocked is not restored; run 'av index <uri>' to fetch it again.".to_string())
        } else {
            Some("That subject was not blocked.".to_string())
        },
    });

    let removed = outcome.removed;
    let unblocked = outcome.unblocked;
    let kind_label = outcome.kind.as_str();
    let subject_label = outcome.subject_id.clone();
    let is_all = subject_label.is_empty();

    print_output(
        cli.non_interactive,
        || {
            if !unblocked {
                println!("{} Nothing was blocked.", console::style("~").yellow());
                return;
            }
            if is_all {
                println!("{} Lifted {removed} block(s).", console::style("✓").green());
                println!(
                    "  {} Deleted resources are not restored — re-index them if you want them back",
                    console::style("~").yellow()
                );
            } else {
                println!(
                    "{} Unblocked {} {}",
                    console::style("✓").green(),
                    kind_label,
                    console::style(&subject_label).cyan()
                );
            }
        },
        &report,
    );

    Ok(())
}
