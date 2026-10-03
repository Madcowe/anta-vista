use av_core::types::{NameRecord, normalize_name};
use rusqlite::{Connection, params};
use std::collections::HashSet;

use crate::{error::IndexResult, filter::SchemeFilter};

#[derive(Debug, Clone)]
pub struct NameResult {
    pub record: NameRecord,
    /// Combined ranking score [0, 1]
    pub score: f32,
    pub trust_score: f32,
    pub agreement_score: f32,
    pub recency_score: f32,
    pub ttl_valid: bool,
}

/// Resolve a name record's target to the local `resource_id` it points at.
///
/// A name stores a URI, while a block is keyed on the content hash in
/// `resources.id`, so relating the two means going through the `resources`
/// row. Returns `Ok(None)` only when the target is genuinely not held locally,
/// in which case there is nothing to check against.
///
/// A *failed* lookup is not the same answer and is returned as `Err` rather than
/// collapsed into it. Swallowing the error would turn a broken `resources` table
/// into "we hold nothing", which reads as "nothing to suppress" and serves
/// exactly the name a block was meant to hide.
fn resolve_target_resource(conn: &Connection, record: &NameRecord) -> IndexResult<Option<String>> {
    // `target` is the form `av index` stores, both run through
    // `av_ingest::location::normalize_uri`. `target_canonical` is the
    // scheme-rewritten form and is the better bet for a record that arrived
    // from a peer, which may still carry the old `ant://` prefix.
    let candidates = [
        Some(record.target.as_str()),
        record.target_canonical.as_deref(),
    ];

    for candidate in candidates.into_iter().flatten() {
        if let Some(resource) = av_store::repo::resources::get_by_location(conn, candidate)? {
            return Ok(Some(resource.id));
        }
    }
    Ok(None)
}

/// Drop name records that point at a blocked resource.
///
/// Blocking a name only covers the claim. Blocking the *resource* it resolves
/// to has to suppress the name too, otherwise a peer that resolves the name
/// through this node is still handed the link that was just blocked — which is
/// the opposite of what "block" means everywhere else here.
///
/// Shared by `lookup_name` and the responder's pre-send gate so both agree.
///
/// Known limit: resolution goes through the `resources` row, so this only
/// covers names whose target we still hold. Once a blocked resource's row is
/// deleted there is nothing left to resolve against, and a claim for that name
/// re-supplied by a peer can be served again.
pub fn retain_unblocked_targets(
    conn: &Connection,
    records: Vec<NameRecord>,
    blocked_resources: &HashSet<String>,
) -> IndexResult<Vec<NameRecord>> {
    if blocked_resources.is_empty() {
        return Ok(records);
    }

    let mut kept = Vec::with_capacity(records.len());
    for r in records {
        // Propagate the error rather than keeping the record on a failed read.
        if let Some(id) = resolve_target_resource(conn, &r)?
            && blocked_resources.contains(&id)
        {
            continue;
        }
        kept.push(r);
    }
    Ok(kept)
}

/// Drop name records that a blocked peer published.
///
/// A peer block stops the node *consulting* that peer, but a name record it sent
/// earlier is already sitting in `name_records` and would otherwise keep
/// resolving and keep being served to other peers. Filtering on
/// `origin_agent_id` — the id the x0x daemon authenticated when the record
/// arrived — is what makes the block actually complete.
///
/// Deliberately *not* filtered on `by_agent_id`: that is the id the sending peer
/// wrote in its own payload, so a peer could stamp an unblocked id onto
/// everything it sends and the block would never fire.
///
/// Records with no origin are kept. That covers the two cases where there is no
/// peer to attribute: records this machine authored via `av name`, and records
/// cached before `origin_agent_id` existed.
pub fn retain_unblocked_agents(
    records: Vec<NameRecord>,
    blocked_agents: &HashSet<String>,
) -> Vec<NameRecord> {
    if blocked_agents.is_empty() {
        return records;
    }
    records
        .into_iter()
        .filter(|r| match &r.origin_agent_id {
            Some(origin) => !blocked_agents.contains(origin),
            None => true,
        })
        .collect()
}

/// Every block that can affect a name record, loaded once per lookup.
///
/// All three kinds interact with a name, and they are independent policies, so
/// they are read together and applied by one function. Keeping the reads in a
/// single place is what stops `lookup_name` and the responder's pre-send gate
/// from drifting apart and one of them quietly serving a suppressed name.
///
/// Fails closed by construction: `load` propagates, and every caller does the
/// same. An unreadable `blocks` table is not evidence that nothing is blocked,
/// and treating it as such would re-expose every name the operator suppressed.
#[derive(Debug, Clone, Default)]
pub struct NameBlockPolicy {
    /// Blocked name record ids.
    pub records: HashSet<String>,
    /// Blocked resource ids, matched against a record's resolved target.
    pub resources: HashSet<String>,
    /// Blocked peer ids, matched against a record's transport origin.
    pub agents: HashSet<String>,
}

impl NameBlockPolicy {
    pub fn load(conn: &Connection) -> IndexResult<Self> {
        Ok(Self {
            records: av_store::repo::blocks::blocked_ids(
                conn,
                av_store::repo::blocks::BlockKind::Name,
            )?,
            resources: av_store::repo::blocks::blocked_ids(
                conn,
                av_store::repo::blocks::BlockKind::Resource,
            )?,
            agents: av_store::repo::blocks::blocked_ids(
                conn,
                av_store::repo::blocks::BlockKind::Agent,
            )?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && self.resources.is_empty() && self.agents.is_empty()
    }
}

/// Apply all three kinds of block to a set of name records.
///
/// The single choke point every name-serving path goes through.
pub fn retain_unblocked(
    conn: &Connection,
    records: Vec<NameRecord>,
    policy: &NameBlockPolicy,
) -> IndexResult<Vec<NameRecord>> {
    let records = records
        .into_iter()
        .filter(|r| !policy.records.contains(&r.record_id))
        .collect();

    let records = retain_unblocked_agents(records, &policy.agents);
    let records = retain_unblocked_targets(conn, records, &policy.resources)?;
    Ok(records)
}

/// Look up a name exactly (case-insensitive via normalized_name).
/// Returns all matching records sorted by ranking score descending.
/// Optionally filtered by scheme.
pub fn lookup_name(
    conn: &Connection,
    name: &str,
    scheme_filter: &SchemeFilter,
    now_secs: i64,
) -> IndexResult<Vec<NameResult>> {
    let normalized = normalize_name(name);
    let mut records = av_store::repo::names::get_by_normalized_name(conn, &normalized)?;

    if records.is_empty() {
        return Ok(vec![]);
    }

    // Local block policy, applied before ranking so blocked records never
    // influence the agreement denominator or the recency range either. Loaded
    // once: `get_by_normalized_name` can return many records for one name.
    //
    // This is what keeps blocked names from being served to peers: the responder
    // answers name queries straight from `lookup_name` and does no filtering of
    // its own afterwards.
    //
    // Fails closed, for the same reason as `search::search_top_k`.
    let policy = NameBlockPolicy::load(conn)?;
    if !policy.is_empty() {
        records = retain_unblocked(conn, records, &policy)?;
        if records.is_empty() {
            return Ok(vec![]);
        }
    }

    // Filter by scheme
    let records: Vec<NameRecord> = records
        .into_iter()
        .map(|mut r| {
            if let Some(ref s) = r.target_scheme {
                r.target_scheme = Some(av_core::types::normalize_scheme(s));
            }
            if let Some(ref tc) = r.target_canonical {
                if tc.starts_with("ant://") {
                    r.target_canonical = Some(tc.replacen("ant://", "autonomi://", 1));
                }
            } else {
                if r.target.starts_with("ant://") {
                    r.target_canonical = Some(r.target.replacen("ant://", "autonomi://", 1));
                } else if r.target.starts_with("autonomi://") {
                    r.target_canonical = Some(r.target.clone());
                }
            }
            r
        })
        .filter(|r| scheme_filter.allows(r.target_scheme.as_deref()))
        .collect();

    if records.is_empty() {
        return Ok(vec![]);
    }

    // Find min/max timestamps for recency normalization
    let (min_ts, max_ts) = records.iter().fold((i64::MAX, i64::MIN), |(mn, mx), r| {
        (mn.min(r.timestamp), mx.max(r.timestamp))
    });
    let ts_range = (max_ts - min_ts).max(1) as f32;

    // Total distinct agents who have published any name record (for agreement denominator)
    let total_agents: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT by_agent_id) FROM name_records",
            [],
            |row| row.get(0),
        )
        .unwrap_or(1)
        .max(1);

    let mut results: Vec<NameResult> = records
        .into_iter()
        .map(|r| {
            let ttl_valid = now_secs < r.timestamp + r.ttl_secs as i64;
            let recency_score = (r.timestamp - min_ts) as f32 / ts_range;

            // Trust: look up the publishing agent's score and normalise [-1,1] → [0,1]
            let trust_score = match av_store::repo::trust::get(conn, &r.by_agent_id) {
                Ok(Some(state)) => ((state.trust_score + 1.0) / 2.0).clamp(0.0, 1.0),
                _ => 0.5,
            };

            // Agreement: fraction of distinct agents who published a record for this name
            let name_agent_count: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT by_agent_id) FROM name_records WHERE normalized_name = ?1",
                    params![&r.normalized_name],
                    |row| row.get(0),
                )
                .unwrap_or(1);
            let agreement_score = (name_agent_count as f32 / total_agents as f32).min(1.0);

            let ttl_validity = if ttl_valid { 1.0_f32 } else { 0.0_f32 };
            let score =
                av_trust::ranking::name_score(trust_score, agreement_score, recency_score, ttl_validity);

            NameResult {
                record: r,
                score,
                trust_score,
                agreement_score,
                recency_score,
                ttl_valid,
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(results)
}
