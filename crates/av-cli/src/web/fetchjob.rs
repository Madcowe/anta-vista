//! Server-side progress for a daemon-fetched Autonomi record.
//!
//! # Why polling and not a streamed response
//!
//! The obvious design is one long response whose body is written as the fetch
//! proceeds, and that is what this replaced. It cannot work here. `tiny_http`
//! writes a response body with `io::copy` through `chunked_transfer::Encoder`,
//! and that encoder **buffers 8 KB and never flushes between reads** — `io::copy`
//! does not call `flush`, and `Encoder::new` sets `flush_after_write: false`. A
//! progress log for a file is far smaller than 8 KB, so every event arrives in
//! one burst when the reader is exhausted, which is the buffered reply wearing a
//! streaming header. Nothing above this module can change that; it is the
//! library.
//!
//! So the fetch runs on its own thread, appends events to a job the page can ask
//! about, and the page polls. At 250 ms over loopback that is invisible next to
//! a lookup that takes seconds, and it has two advantages a stream would not: no
//! worker stays occupied for the length of the download, and a fetch survives a
//! page that reloads or closes, because the events outlive the connection.
//!
//! # What the page may see
//!
//! A job id is a uuid v4 and the *only* thing that addresses one, which is the
//! same authorisation model as the viewer's unguessable token path: there is no
//! cookie and no session, and guessing a v4 is not a strategy. The events
//! themselves are server-authored status text and counts from the transports;
//! no peer bytes are ever stored here — those go to the viewer port.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;

/// One thing that happened during a fetch.
///
/// Serialised to the page as the tagged form it already knows: `type` plus the
/// fields for that variant. The names here are the wire contract.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FetchEvent {
    /// A coarse step, named so the page can say which transport is running.
    Phase {
        message: String,
        transport: Option<String>,
    },
    /// One transport's own wording. Prose from the network, so the page treats
    /// it as text and never as markup.
    Status { message: String },
    /// A chunk that arrived, with running totals.
    ///
    /// `chunks` and `total_bytes` count *distinct retrieved chunks*, not attempts
    /// and not a fraction of the file. Self-encryption padding means the byte
    /// total is never the file size, which is why the page shows a count and a
    /// byte figure and never a percentage.
    Chunk {
        index: u64,
        bytes: u64,
        chunks: u64,
        total_bytes: u64,
        retried: bool,
    },
    /// The fetch finished and the bytes are being served.
    Done {
        url: String,
        mime_type: String,
        filename: String,
        bytes: u64,
        expires_in: u64,
        transport: String,
        /// Whether a browser can display this, or will only download it.
        ///
        /// Not decoration: the page uses it to choose between navigating the tab to
        /// `url` and downloading the file instead. A tab pointed at a viewer URL
        /// serving `Content-Disposition: attachment` sits there saying it is opening
        /// the file while the browser quietly downloads it instead — so the page has
        /// to be able to ask, and this is the same `is_browser_renderable` verdict the
        /// browser path gets from `/api/view/plan`. Two answers to one question from
        /// two transports is exactly how they drift.
        renderable: bool,
    },
    /// The fetch failed, or never started.
    Error { message: String },
}

/// How long a job and its events are kept after it finishes.
///
/// Matches the viewer's own lifetime, so a job cannot outlive the token it leads
/// to. Also what bounds the map: a page that starts a fetch and walks away
/// leaves at most this much behind.
const RETAIN: std::time::Duration = std::time::Duration::from_secs(180);

/// Never let one job's event log grow without bound.
///
/// The ant CLI emits a line per chunk and a large record can have thousands. The
/// page shows a rolling window, so keeping every event forever would be memory
/// spent on detail nobody will read.
const MAX_EVENTS: usize = 512;

/// A fetch in progress, or finished and waiting to be collected.
struct Job {
    /// Sequenced so a cursor means "everything from here on" rather than "index
    /// `since` into a list that shrinks".
    events: VecDeque<SequencedEvent>,
    /// The next sequence number to hand out. This is the cursor a poll returns,
    /// and it never goes backwards — which is the whole reason the log can be
    /// trimmed without stranding a page that is still reading it.
    next_seq: u64,
    /// Set once the fetch thread is done, whether it succeeded or failed. The
    /// page stops polling on this, which is what ends the loop.
    finished: bool,
    /// When the job was created, for [`RETAIN`].
    created: Instant,
}

/// One event and its position in the job's log.
struct SequencedEvent {
    seq: u64,
    event: FetchEvent,
}

static JOBS: Mutex<Option<HashMap<String, Job>>> = Mutex::new(None);

/// A job id for a new fetch.
///
/// A v4 UUID, because `uuid` is already a dependency for message ids and the
/// property that matters here is that another site cannot walk the job table: the
/// ids are in URLs the browser polls, and a job's events name the resource being
/// fetched. A counter or a timestamp is guessable — and the page is a loopback
/// listener that a page in another tab could script — so this is not the place
/// to economise on entropy.
///
/// The format is not load-bearing: [`view_fetch_status`](super::handlers) accepts
/// a lowercase hex string, which a UUID without its dashes is.
fn next_job_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn start() -> String {
    let id = next_job_id();
    let Some(mut guard) = lock() else { return id };
    let map = guard.get_or_insert_with(HashMap::new);
    prune(map);
    map.insert(
        id.clone(),
        Job {
            events: VecDeque::new(),
            next_seq: 0,
            finished: false,
            created: Instant::now(),
        },
    );
    id
}

/// Append an event. A no-op once the job is finished, so a late event from a
/// thread that overran cannot contradict the result the page already read.
pub fn push(id: &str, event: FetchEvent) {
    let Some(mut jobs) = lock() else { return };
    let Some(job) = jobs.get_or_insert_with(HashMap::new).get_mut(id) else {
        // The job expired while a slow fetch was still running. Nothing to report
        // to; the page will have seen its own timeout.
        return;
    };
    if job.finished {
        return;
    }
    let seq = job.next_seq;
    job.next_seq += 1;
    job.events.push_back(SequencedEvent { seq, event });
    if job.events.len() > MAX_EVENTS {
        // Drop from the front: the newest events are the ones still being read,
        // and the page's own window discards old lines anyway. Safe to do only
        // because the cursor is a sequence number rather than an index — a page
        // still holding an older cursor keeps getting the events that follow its
        // gap instead of silently receiving nothing for the rest of the fetch.
        let excess = job.events.len() - MAX_EVENTS;
        job.events.drain(..excess);
    }
}

/// Mark a job finished, so the page stops polling.
pub fn finish(id: &str) {
    let Some(mut jobs) = lock() else { return };
    if let Some(job) = jobs.get_or_insert_with(HashMap::new).get_mut(id) {
        job.finished = true;
    }
}

/// What one poll returns.
#[derive(Debug, Serialize)]
pub struct Snapshot {
    /// Events the page has not seen, in order.
    pub events: Vec<FetchEvent>,
    /// The cursor to send with the next poll.
    ///
    /// The page must take this rather than counting the events it was sent: the
    /// count is only equal to the cursor while nothing has been trimmed, and it is
    /// the equality that quietly stops holding on a long fetch.
    pub next: u64,
    /// True once the fetch is over, whatever happened. The page stops polling on
    /// this, so it must be true even when the last event is an error.
    pub done: bool,
}

/// Read a job's unseen events.
///
/// `since` is the count the page has already seen, so events are delivered once
/// and in order. Returns `None` for a job that does not exist — either expired or
/// never started, which the page cannot tell apart and does not need to.
pub fn read(id: &str, since: u64) -> Option<Snapshot> {
    let mut jobs = lock()?;
    let map = jobs.get_or_insert_with(HashMap::new);
    prune(map);
    let job = map.get(id)?;
    Some(Snapshot {
        events: job
            .events
            .iter()
            .filter(|sequenced| sequenced.seq >= since)
            .map(|sequenced| sequenced.event.clone())
            .collect(),
        next: job.next_seq,
        done: job.finished,
    })
}

/// Forget jobs past [`RETAIN`].
///
/// Called from [`start`] and [`read`], which are the only entry points, so the
/// map cannot grow without an operation that also prunes it.
fn prune(map: &mut HashMap<String, Job>) {
    map.retain(|_, job| job.created.elapsed() < RETAIN);
}

fn lock() -> Option<std::sync::MutexGuard<'static, Option<HashMap<String, Job>>>> {
    // A poisoned lock means a fetch thread panicked while holding it. Recovering
    // would silently discard the events of jobs that are still running, so a
    // failure here reads as "no job" rather than as an empty job.
    JOBS.lock().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(index: u64) -> FetchEvent {
        FetchEvent::Chunk {
            index,
            bytes: 10,
            chunks: index,
            total_bytes: 10 * index,
            retried: false,
        }
    }

    #[test]
    fn events_are_delivered_once_and_in_order() {
        let id = start();
        push(
            &id,
            FetchEvent::Status {
                message: "one".into(),
            },
        );
        push(&id, chunk(1));
        push(
            &id,
            FetchEvent::Status {
                message: "two".into(),
            },
        );
        finish(&id);

        let first = read(&id, 0).expect("job exists");
        assert_eq!(first.events.len(), 3);
        assert!(first.done);
        assert!(matches!(first.events[0], FetchEvent::Status { .. }));
        assert!(matches!(first.events[1], FetchEvent::Chunk { .. }));

        // Asking again from the same cursor repeats nothing, which is what lets
        // the page resume after a dropped poll without re-showing events.
        assert_eq!(read(&id, 3).expect("job exists").events.len(), 0);
        assert_eq!(read(&id, 1).expect("job exists").events.len(), 2);
    }

    #[test]
    fn a_job_is_not_done_until_it_is_finished() {
        // The page's polling loop turns on this: without it, a slow fetch would
        // spin forever on the last event.
        let id = start();
        push(
            &id,
            FetchEvent::Status {
                message: "working".into(),
            },
        );
        assert!(!read(&id, 0).expect("job exists").done);
        finish(&id);
        assert!(read(&id, 0).expect("job exists").done);
    }

    #[test]
    fn an_unknown_job_is_absent_rather_than_empty() {
        // Absent and "finished with nothing to say" mean opposite things to the
        // page: one is a lost job, the other a fetch that ended quietly.
        assert!(read("not-a-job", 0).is_none());
    }

    #[test]
    fn no_event_outlives_the_result_it_would_contradict() {
        // A thread that overran must not be able to append a status line after
        // the `done` the page has already acted on.
        let id = start();
        finish(&id);
        push(
            &id,
            FetchEvent::Status {
                message: "too late".into(),
            },
        );
        assert!(read(&id, 0).expect("job exists").events.is_empty());
    }

    #[test]
    fn the_event_log_is_bounded_and_keeps_the_newest() {
        let id = start();
        for i in 1..=(MAX_EVENTS as u64 + 50) {
            push(&id, chunk(i));
        }
        let all = read(&id, 0).expect("job exists").events.len();
        assert_eq!(all, MAX_EVENTS, "the log must not grow without bound");
        // The newest survived, which is the one the page is waiting for.
        match all_events(&id).last() {
            Some(FetchEvent::Chunk { index, .. }) => {
                assert_eq!(*index, MAX_EVENTS as u64 + 50);
            }
            other => panic!("expected the last chunk, got {other:?}"),
        }
    }

    /// A cursor must survive the log being trimmed underneath it.
    ///
    /// With an index-into-a-Vec cursor, a page that had seen 50 events then reads
    /// 600 more off a job capped at 512 gets `events[50..]` off a 512-long list
    /// that has already lost its first 50 — so it is handed the tail of the log
    /// *again*, and the tab fills with duplicates while the fetch runs on. If the
    /// cap ever exceeds what has been consumed, the same arithmetic instead
    /// returns nothing at all: the page stops receiving progress, sees
    /// `done: false`, and polls a finished fetch forever. A sequence number is
    /// what makes "everything after this point" survive a shrinking list.
    #[test]
    fn a_cursor_still_works_after_the_log_has_been_trimmed() {
        let id = start();
        // A first poll that sees a couple of events, as a page would.
        push(&id, chunk(1));
        push(&id, chunk(2));
        let first = read(&id, 0).expect("job exists");
        assert_eq!(first.events.len(), 2);

        // Far more than `MAX_EVENTS`, so the first two are long gone from the log.
        for i in 3..=(MAX_EVENTS as u64 * 3) {
            push(&id, chunk(i));
        }

        let resumed = read(&id, first.next).expect("job exists");
        assert!(
            !resumed.events.is_empty(),
            "the page must still receive the events that follow its gap"
        );
        // Nothing it already saw comes back.
        for event in &resumed.events {
            match event {
                FetchEvent::Chunk { index, .. } => assert!(
                    *index >= 3,
                    "chunk {index} was already delivered before the trim"
                ),
                other => panic!("unexpected {other:?}"),
            }
        }
        // And the newest survives the resume, which is the whole point.
        match resumed.events.last() {
            Some(FetchEvent::Chunk { index, .. }) => {
                assert_eq!(*index, MAX_EVENTS as u64 * 3)
            }
            other => panic!("expected the last chunk, got {other:?}"),
        }
    }

    /// A poll from a cursor at or past the end is empty but still reports where to
    /// resume, so a page that over-counts cannot wedge itself: the next poll for
    /// the *next* new event still arrives.
    #[test]
    fn a_cursor_past_the_end_recovers_on_the_next_event() {
        let id = start();
        push(&id, chunk(1));
        push(&id, chunk(2));
        let ahead = read(&id, 10_000).expect("job exists");
        assert!(ahead.events.is_empty());
        assert_eq!(ahead.next, 2, "the cursor still points at the log's end");

        // A page stuck on a cursor past the end would never see this. The server
        // cannot tell "over-counted" from "caught up", so it hands back the tail
        // instead of an empty page forever.
        push(&id, chunk(3));
        let back = read(&id, 0).expect("job exists");
        assert_eq!(back.events.len(), 3);
    }

    /// Everything still held, ignoring the cursor. Only for the bound check
    /// above, where the point is what the job retained rather than what one poll
    /// would return.
    fn all_events(id: &str) -> Vec<FetchEvent> {
        read(id, 0).expect("job exists").events
    }

    #[test]
    fn job_ids_are_not_guessable_from_each_other() {
        let ids: Vec<String> = (0..64).map(|_| start()).collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "job ids must not repeat");
        assert!(
            ids.iter().all(|id| id.len() >= 32),
            "a short id is not a secret"
        );
    }
}
