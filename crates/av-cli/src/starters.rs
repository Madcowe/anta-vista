//! Bundled starter resources for a fresh install.
//!
//! Three rows ship with the binary — `https://autonomi.com/`,
//! `https://x0x.md/` and a dog photo on Autonomi — so the interface is not
//! empty on first open. Their descriptors and MiniLM vectors are **baked at
//! build time** (regenerate with `cargo test -p av-cli --test bake_starters
//! -- --ignored --nocapture`), which is what makes seeding free: it is a
//! handful of local INSERTs with no network and no model load, so it runs
//! synchronously before the web server binds and the status drawer already
//! reports the rows at first paint.
//!
//! The rules [`ensure_seeded`] enforces, each for a reason a user would
//! notice:
//!
//! - A location with a marker row in `starter_seeds` is settled forever, so
//!   a purged starter stays purged across restarts (migration 008 explains
//!   why the marker lives in its own table).
//! - A location that already holds a resource is *adopted*, not overwritten:
//!   if the user indexed their own copy before the first boot, theirs wins.
//! - A blocked content hash skips the write and leaves the marker unset, so
//!   `av unblock` can still let the starter land on a later boot. Block reads
//!   fail closed — an unreadable block list aborts the whole pass rather than
//!   seeding over it.
//! - Any error aborts the pass and is logged by the caller; a nicety must
//!   never keep the interface from starting.
//!
//! [`seeding_disabled`] (`ANTA_VISTA_NO_SEED=1`/`true`/`yes`) turns the whole
//! thing off. The web test harness sets it because the search tests assert
//! exact result sets on fresh databases.
//!
//! Scope: this runs from `av web` only. The default bare-`av` launch is the
//! web interface, which covers a fresh install; wiring it into more CLI
//! commands is possible later but means gating more test harnesses.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use av_core::types::{EmbeddingRecord, ResourceDescriptor};
use av_embed::provider::{minilm_profile, profile_id};

use crate::cmd::{CliError, CliResult};
pub use crate::starters_data::STARTERS;

/// Environment opt-out from starter seeding.
///
/// Same spellings as [`crate::startup::NO_AUTOSTART_ENV`]: `1`, `true` or
/// `yes`.
pub const NO_SEED_ENV: &str = "ANTA_VISTA_NO_SEED";

pub fn seeding_disabled() -> bool {
    matches!(
        std::env::var(NO_SEED_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// What one [`ensure_seeded`] pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SeedReport {
    /// Starters this pass inserted (resource + embedding + marker).
    pub seeded: usize,
    /// Locations that already held the starter: marker written, the existing
    /// row left exactly as it was.
    pub adopted: usize,
    /// Starters skipped because their content hash is blocked. Deliberately
    /// left unmarked so an unblock can still let them land.
    pub blocked: usize,
}

/// Seed the bundled starter resources into the database at `db_path`.
///
/// Idempotent and safe to call on every boot: already-settled locations are
/// skipped after one marker read, so the steady-state cost is three tiny
/// SELECTs. See the module docs for the ordering rules.
pub fn ensure_seeded(db_path: &Path) -> CliResult<SeedReport> {
    let conn = av_store::open(db_path).map_err(|e| CliError::Database(e.to_string()))?;
    let profile = minilm_profile();
    let pid = profile_id(&profile);
    let mut report = SeedReport::default();

    // One transaction for the whole pass, and `BEGIN IMMEDIATE` rather than a
    // deferred BEGIN: this runs single-threaded before the bind, but the rule
    // for anything that reads then writes still applies — an upgrade can lose
    // the lock to another connection (the pre-warm above, or a test's own
    // handle) with SQLITE_BUSY_SNAPSHOT. The point of the transaction is
    // atomicity per starter: a crash between the resource row and its
    // embedding would leave a half-seeded location that the next boot would
    // "adopt" as complete — marked forever, invisible to search. On any error
    // the connection drops with the transaction open, which rolls it back.
    conn.execute("BEGIN IMMEDIATE", [])
        .map_err(|e| CliError::Database(e.to_string()))?;

    for starter in STARTERS {
        let resource: ResourceDescriptor = serde_json::from_str(starter.resource_json)
            .map_err(|e| CliError::Database(format!("baked starter data failed to parse: {e}")))?;

        if starter.vector.len() != profile.dim as usize {
            return Err(CliError::Database(format!(
                "baked starter for {} has a {}-dim vector, expected {}",
                resource.location,
                starter.vector.len(),
                profile.dim
            )));
        }

        if av_store::repo::starter_seeds::is_seeded(&conn, &resource.location)
            .map_err(|e| CliError::Database(e.to_string()))?
        {
            continue;
        }

        // The user got there first — their row ships, and the marker stops
        // us from ever touching this location again.
        if av_store::repo::resources::get_by_location(&conn, &resource.location)
            .map_err(|e| CliError::Database(e.to_string()))?
            .is_some()
        {
            av_store::repo::starter_seeds::mark_seeded(&conn, &resource.location, now_secs())
                .map_err(|e| CliError::Database(e.to_string()))?;
            report.adopted += 1;
            continue;
        }

        // Fail closed, like the index pipeline: an unreadable block list is
        // an error, not evidence that nothing is blocked. Skipped stays
        // unmarked so `av unblock` remains able to admit it later.
        if av_store::repo::blocks::is_blocked(
            &conn,
            av_store::repo::blocks::BlockKind::Resource,
            &resource.id,
        )
        .map_err(|e| CliError::Database(format!("could not read the block list: {e}")))?
        {
            report.blocked += 1;
            continue;
        }

        av_store::repo::resources::insert(&conn, &resource)
            .map_err(|e| CliError::Database(e.to_string()))?;
        av_store::repo::embeddings::insert_profile(&conn, &pid, &profile)
            .map_err(|e| CliError::Database(e.to_string()))?;

        let vector = starter.vector.to_vec();
        // Same norm the indexing pipeline stores, computed the same way.
        let l2_norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        av_store::repo::embeddings::insert(
            &conn,
            &EmbeddingRecord {
                resource_id: resource.id.clone(),
                profile_id: pid.clone(),
                vector,
                l2_norm,
                // The baked descriptor's own timestamp: deterministic across
                // installs, and nothing ranks or displays by creation time.
                created_at: resource.created_at,
            },
        )
        .map_err(|e| CliError::Database(e.to_string()))?;

        av_store::repo::starter_seeds::mark_seeded(&conn, &resource.location, now_secs())
            .map_err(|e| CliError::Database(e.to_string()))?;
        report.seeded += 1;
    }

    conn.execute("COMMIT", [])
        .map_err(|e| CliError::Database(e.to_string()))?;
    Ok(report)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One baked starter: the stored row as JSON, and the MiniLM vector of its
/// description. Defined here so the generated data file has one obvious
/// dependency direction: data -> this type -> the seeder.
pub struct BakedStarter {
    /// `serde_json` of the [`ResourceDescriptor`] exactly as it should be
    /// inserted.
    pub resource_json: &'static str,
    /// The description's embedding under [`minilm_profile`].
    pub vector: &'static [f32],
}
