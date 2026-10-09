/* Anta-Vista web interface.
 *
 * All state comes from the JSON API; nothing is persisted server-side beyond
 * the database writes the CLI already performs. The only client-side state is
 * the theme, kept in localStorage.
 */
'use strict';

const THEME_KEY = 'anta-vista-theme';
/* Warnings the user has told us to stop repeating. Kept in localStorage so the
 * choice survives a reload, alongside the theme. */
const MUTE_KEY = 'anta-vista-muted-warnings';

function mutedWarnings() {
  try {
    return new Set((localStorage.getItem(MUTE_KEY) || '').split(',').filter(Boolean));
  } catch (e) {
    return new Set();
  }
}

function muteWarning(id) {
  try {
    const set = mutedWarnings();
    set.add(id);
    localStorage.setItem(MUTE_KEY, [...set].join(','));
  } catch (e) {
    /* storage unavailable: the warning simply comes back next time */
  }
}

const el = (id) => document.getElementById(id);

const ui = {
  themeToggle: el('theme-toggle'),
  statusBtn: el('open-status'),
  statusDrawer: el('status-drawer'),
  statusBody: el('status-body'),
  closeStatus: el('close-status'),
  stopListener: el('stop-listener'),
  overlay: el('overlay'),
  banner: el('setup-banner'),
  form: el('search-form'),
  q: el('q'),
  results: el('results'),
  resultsSection: el('results-section'),
  resultsCount: el('results-count'),
  resultsEmpty: el('results-empty'),
  resultsNote: el('results-note'),
  addForm: el('add-form'),
  addStatus: el('add-status'),
  template: el('result-template'),
  dialog: el('install-dialog'),
  dialogTitle: el('install-title'),
  dialogBlurb: el('install-blurb'),
  dialogCommand: el('install-command'),
  dialogConfirm: el('install-confirm'),
  blockDialog: el('block-dialog'),
  blockKeep: el('block-dialog').querySelector('[data-role="block-keep"]'),
  blockDelete: el('block-dialog').querySelector('[data-role="block-delete"]'),
  blockTitle: el('block-title'),
  blockBlurb: el('block-blurb'),
  recommendDialog: el('recommend-dialog'),
  peerBlockDialog: el('peer-block-dialog'),
  peerBlockBlurb: el('peer-block-blurb'),
  removeDialog: el('remove-dialog'),
  removeTitle: el('remove-title'),
  removeBlurb: el('remove-blurb'),
  dialogRun: el('install-run'),
};

/* The query the current results belong to. Ratings are boosted per exact
 * query, so this has to travel with the result. */
let lastQuery = '';

/* ---------------------------------------------------------------- theme -- */

function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  /* The label always names **what the button is showing right now**, and
   * clicking leaves it: on the 90s it offers to leave them, on modern it offers
   * to leave now. The previous pairing said "I don't like the 90's" while
   * pointing *at* the 90s, so on the default theme the button read as an escape
   * and went the other way. The static markup carries the same string for the
   * default so there is no flash of a wrong label before this runs. */
  ui.themeToggle.textContent =
    theme === 'retro' ? 'I don\u2019t like the 90\u2019s' : 'I don\u2019t like now';
  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch (e) {
    /* storage unavailable: the theme still applies for this page */
  }
}

ui.themeToggle.addEventListener('click', () => {
  applyTheme(document.documentElement.dataset.theme === 'retro' ? 'modern' : 'retro');
});

/* ------------------------------------------------------------------ api -- */

async function api(path, options) {
  const res = await fetch(path, options);
  let body = null;
  try {
    body = await res.json();
  } catch (e) {
    throw new Error(`${res.status} ${res.statusText}`);
  }
  if (!res.ok || body.ok === false) {
    const err = new Error(body.detail || `${res.status} ${res.statusText}`);
    err.code = body.error;
    /* Keep the rest of the error body reachable. Some failures carry the exact
     * action to take, such as the platform-specific install command, and the UI
     * has to show it rather than invent one. */
    err.body = body;
    throw err;
  }
  return body;
}

const postJSON = (path, payload) =>
  api(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });

/* ------------------------------------------------------------ autonomi fetch -- */

/* Mirrors the server's MAX_BODY. The server rejects an oversized body rather
 * than truncating it, but discovering that after a fetch that may have taken
 * half a minute is a poor trade, so the check happens before the upload. */
const MAX_UPLOAD = 32 * 1024 * 1024;

/* The two binary endpoints take the file as the request body, so the location
 * rides in a header. Both are same-origin POSTs from this page, which is what
 * the server's cross-site guard keys on. */
const postBlob = (path, uri, blob, extra = {}) =>
  api(path, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/octet-stream',
      'X-Anta-Uri': uri,
      ...extra,
    },
    body: blob,
  });

/* Mirrors the server's PLAN_PREFIX_CAP. Bytes decide the type in the first few
 * hundred, so this is generous rather than tuned; it exists to keep the request
 * small, not to cut a signal short. `slice` caps it, so the page cannot send
 * more however large the record is. */
const PLAN_PREFIX = 16 * 1024;

/* Ask the server what a file should be called and whether a browser can show it.
 *
 * Sends the leading PLAN_PREFIX bytes, not the file. The point of this call is
 * to avoid uploading a file already sitting in memory, and the Autonomi SDK
 * reports `application/octet-stream` for most records — a DataMap chunk carries
 * no content type — so trusting that type alone sent real images and PDFs to a
 * download. A prefix is enough to see magic bytes and an HTML marker, which is
 * what the verdict actually turns on.
 *
 * The renderable verdict is computed server-side from `is_browser_renderable`,
 * because the viewer independently applies the same policy when it picks
 * `inline` versus `attachment` — two copies of "can a browser display this?"
 * would drift, and the drift would show up as a blank window. */
const postPlan = (uri, mimeType, blob) =>
  api('/api/view/plan', {
    method: 'POST',
    headers: {
      'X-Anta-Uri': uri,
      'X-Anta-Mime': mimeType || '',
      'Content-Type': 'application/octet-stream',
    },
    body: blob ? blob.slice(0, PLAN_PREFIX) : null,
  });

/* Hand bytes straight to the browser as a file, named by the server.
 *
 * A Blob the page already holds needs no server round-trip and no size limit;
 * the object URL is the only way to get at it. Revoking on the next tick rather
 * than synchronously matters: revoking it immediately can cancel the download
 * in some browsers before it has started reading. */
function downloadBlob(blob, filename) {
  const url = URL.createObjectURL(blob);
  downloadUrl(url, filename);
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

/* Save something the browser is being sent as an attachment.
 *
 * A plain link click rather than `window.open`, because the point is that the tab
 * should *not* navigate: the server sends `Content-Disposition: attachment` for a
 * type it cannot display, and navigating to it leaves a tab sitting on the progress
 * page while the file lands in the downloads folder.
 *
 * Kept separate from `downloadBlob` because the two sources are genuinely
 * different: the browser path already holds the bytes and can hand them over
 * directly, while the daemon path has only a viewer URL. The `download` attribute
 * is honoured for same-origin URLs, which this is. */
function downloadUrl(url, filename) {
  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  link.click();
}

const AUTONOMI_SCHEME = /^autonomi:\/\//i;

/* The record address an `autonomi://` location refers to: bare 64 hex.
 *
 * The SDK rejects a scheme-prefixed value with a message about expecting 32
 * hexadecimal bytes, so the scheme has to come off here. A trailing `/name`
 * segment is the author's label for the file, not part of the address, so it is
 * dropped too — `?name=` is the query-string form of the same idea.
 *
 * Returns null when the location is not an Autonomi address this SDK could ask
 * for, so callers can keep treating it as an ordinary link. */
function autonomiAddress(location) {
  if (!AUTONOMI_SCHEME.test(location)) return null;
  const rest = location
    .replace(AUTONOMI_SCHEME, '')
    .split(/[/?#]/)[0]
    .toLowerCase();
  return /^[0-9a-f]{64}$/.test(rest) ? rest : null;
}

let sdkPromise = null;

/* Load the vendored Autonomi browser library once, on first use.
 *
 * Lazy because the library is mostly WASM and most sessions never touch an
 * Autonomi link, so its cost should not land on page load. The result is cached
 * *including* a failure: a browser that cannot run it should say so once, not
 * on every click, and a transient network blip should not leave a permanently
 * poisoned cache. */
function loadSdk() {
  if (!sdkPromise) {
    const base = window.__AV_SDK_BASE__ || '/assets/vendor/autonomi-sdk/0.1.1/';
    sdkPromise = import(`${base}index.js`).then(
      (mod) => ({ mod }),
      (err) => ({ error: `the Autonomi browser library could not be loaded (${err.message})` })
    );
  }
  return sdkPromise;
}

/* Whether this browser can fetch from the Autonomi network at all.
 *
 * The SDK checks the pieces it needs — WebAssembly, workers, IndexedDB, and
 * above all a usable WebRTC data channel — and reports which operation is
 * unavailable. Asking once and explaining beats offering an Open button that
 * fails, since the failure modes (a file:// page, no WebRTC) are not things the
 * user can fix by clicking harder. */
async function autonomiCapability() {
  const { mod, error } = await loadSdk();
  if (error) return { available: false, reason: error };
  const caps = (mod.getBrowserCapabilities && mod.getBrowserCapabilities()) || {};
  const download = (caps.operations && caps.operations.download) || {};
  if (download.available === false) {
    return {
      available: false,
      reason:
        download.reason ||
        'this browser cannot reach the Autonomi network, so files cannot be fetched here',
    };
  }
  return { available: true };
}

/* Every label is phrased as something the user is *waiting for*, never as
 * something that has happened. The headline already guarantees this structurally
 * by prefixing "Fetching from Autonomi", so these only need to avoid contradicting
 * it: "Downloading done" read as finished while the transfer was still running,
 * and "Preparing the download" put the word *download* in the user's mouth at a
 * point where nothing had been downloaded at all.
 *
 * There is deliberately **no `complete` and no `failed`** here. The SDK's `complete`
 * marks an *operation boundary*, not the user's download: `#openReader` reports it
 * the instant the reader object exists — `report(\`Opened ...\`, { phase: "complete" })`
 * before returning, with not one byte yet moved. Rendering that said "Fetch complete"
 * for the whole of a large transfer, which is exactly backwards. Whether the bytes
 * are here is ours to say: the stream below counts them, and `done`/`error` are the
 * only terminal states the page will show. */
const PHASE_LABELS = {
  initializing: 'Loading the Autonomi library',
  connecting: 'Connecting to nodes',
  lookup: 'Finding the nodes holding this file',
  preparing: 'Preparing to fetch',
  downloading: 'Transferring the file',
};

/* How much plaintext to pull per read when streaming a record.
 *
 * The SDK's own default is 1 MiB and its hard cap is 4 MiB, so this only has to be
 * valid, not clever. Larger reads mean fewer round trips and a smoother count;
 * each read is a network request, so an 8 MiB ask would be rejected outright. */
const READ_CHUNK_BYTES = 1024 * 1024;

const formatBytes = (n) => {
  if (!Number.isFinite(n)) return '';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
};

/* Progress events arrive from both calls below, so the handler is shared: a
 * phase with no byte counts gets a plain label, and one with them gets
 * `n of total`. An event missing `phase` must still produce text, because a
 * blank status line reads as a hung page.
 *
 * `event.message` is the SDK's own wording and is built from what the network
 * answered, so it is worth preferring when present — it distinguishes "waiting on
 * node 3 of 7" from "waiting on node 1 of 7" — but it is never the only thing
 * shown, because a peer that chose to say nothing would otherwise leave the line
 * blank. The phase label is the floor, not the ceiling. */
function progressLine(event) {
  /* Our own label, or a neutral fallback — **never the SDK's `message`**.
   *
   * The SDK narrates itself in words built from what the network answered, which
   * includes names that mean nothing to a user: opening a record reports
   * "opened public-file-3f9a2c71", because the SDK always synthesises that filename
   * and we already refuse to trust it for anything (see the download-name policy).
   * Surfacing it next to our own counts read as though the app had picked a file
   * off the network. A phase we have a word for is ours; anything else is just
   * "Fetching…". */
  const label = PHASE_LABELS[event.phase] || 'Fetching…';
  const completed = Number(event.completed);
  const total = Number(event.total);
  if (Number.isFinite(completed) && Number.isFinite(total) && total > 0) {
    return `${label} \u2014 ${formatBytes(completed)} of ${formatBytes(total)}`;
  }
  return label;
}

function reportSdkProgress(tab, statusEl, event) {
  /* The SDK's progress drives the *headline* only.
   *
   * Its byte counts are deliberately dropped: we already have authoritative ones
   * from the stream we are consuming — chunks read and bytes against the size the
   * DataMap promised. Sending both to the same read-out made the line flicker
   * between "Downloading — 1.2 MB of 8.0 MB" and "12 chunks · 1.4 MB of 8.0 MB"
   * as each fired, which reads as a broken counter rather than a progress one. One
   * source of truth, so the numbers move in one direction only. */
  /* `complete` / `failed` are boundaries of an SDK *operation*, not of the
   * download the user is waiting for — see `PHASE_LABELS`. Letting one through
   * would report "the file is here" while the stream is still pulling it. Our own
   * `onStep` announces the transfer starting, and `done`/`error` end it. */
  if (event.phase === 'complete' || event.phase === 'failed') return;
  const message = progressLine(event);
  setStatus(statusEl, message, '');
  toTab(tab, { type: 'phase', message });
}

function openProgressTab() {
  const tab = window.open('/view/progress', '_blank');
  if (!tab) return tab;

  /* Updates are **queued and flushed**, not awaited.
   *
   * Holding a promise and having `toTab` await it before posting has one failure
   * mode and it is total: if the page's `ready` message never arrives — a lost
   * post, a browser that dropped it, a script that threw before posting — every
   * single update is awaited forever and silently discarded. The tab then shows
   * its opening sentence for the whole fetch with no error anywhere, which is
   * indistinguishable from a fetch that reports nothing.
   *
   * So nothing is ever awaited. Updates queue, the page flushes them the moment it
   * says it is listening, and a timer flushes them anyway if it never says. Order
   * is preserved either way, and a late page loses at most the events sent before
   * it was listening rather than all of them.
   */
  tab.__avQueue = [];
  tab.__avListening = false;
  const flush = () => {
    if (tab.__avListening) return;
    tab.__avListening = true;
    for (const queued of tab.__avQueue.splice(0)) {
      tab.postMessage(queued, window.location.origin);
    }
  };
  tab.addEventListener('message', (event) => {
    if (event.origin !== window.location.origin) return;
    if (event.data && event.data.type === 'ready') flush();
  });
  // The failsafe. A page that never answers must not leave the queue growing.
  setTimeout(flush, PROGRESS_HANDSHAKE_MS);
  return tab;
}

/* How long to wait for the page to say it is listening before giving up on it.
 *
 * Generous enough for a cold parse of the page on a slow machine, short enough
 * that a broken channel degrades into "updates are dropped" rather than "the
 * fetch looks hung". */
const PROGRESS_HANDSHAKE_MS = 1500;

/* Sever the opener relationship, before peer-supplied content can reach it.
 *
 * `window.open`'s `noopener` feature cannot be used for this, because it makes
 * the call return null and the handle is what the finished file is loaded into.
 * Cross-origin means the fetched document cannot *read* the app, but it can
 * *navigate* it — and peer-supplied HTML is exactly that untrusted input. */
function cutOpener(tab) {
  if (tab) tab.opener = null;
}

function toTab(tab, message) {
  if (!tab) return;
  try {
    if (tab.__avListening) {
      tab.postMessage(message, window.location.origin);
      return;
    }
    /* Bounded: a fetch that reports faster than a page can be expected to load
     * must not grow this without limit. The oldest are dropped, because the
     * newest are the ones that describe where the transfer has got to. */
    tab.__avQueue.push(message);
    if (tab.__avQueue.length > PROGRESS_QUEUE_MAX) tab.__avQueue.shift();
  } catch (_) {
    /* The tab was closed, or navigated away. The fetch still finishes; there is
     * simply nowhere left to report to. */
  }
}

/* How many updates may wait for a page that has not said it is listening. */
const PROGRESS_QUEUE_MAX = 64;

/* ------------------------------------------------------------ daemon fetch -- */

/* Have the server fetch a location, reporting progress while it runs.
 *
 * Two requests, not one: a POST that starts the fetch and returns a job id, then
 * polls for events. The obvious alternative is a single streamed response, and it
 * cannot work against this server — tiny_http copies a response body through a
 * chunked encoder that buffers 8 KB and never flushes, so every event would
 * arrive in one burst when the fetch ended. That is the blank tab again, with a
 * streaming header on it. `fetchjob` carries the argument.
 *
 * The POST is what inherits the server's cross-site guard, and it is also where
 * every refusal happens: a blocked location and a missing backend are both
 * answered before a thread starts, so they arrive as a status and a code rather
 * than as a progress log that later turns into an error. That is why a non-2xx
 * here means "the server will not do this", which the caller turns into a
 * `DaemonFallback` rather than an error it has to display.
 *
 * `onEvent` sees every event, and the tab is written to alongside it. */
async function fetchViaDaemon(uri, { tab, onEvent } = {}) {
  let started;
  try {
    started = await api('/api/view/fetch', {
      method: 'POST',
      headers: { 'X-Anta-Uri': uri },
    });
  } catch (err) {
    const code = err.code || 'fetch_failed';
    const detail = (err.body && err.body.detail) || err.message;
    // 503 and 403 are refusals rather than failures: the server is there and has
    // declined. Worth handing back as a routing decision so the caller can offer
    // something else.
    if (code === 'no_download_backend' || code === 'blocked') {
      throw new DaemonFallback(detail);
    }
    throw err;
  }

  const statusPath = started.status || `/api/view/fetch/${started.job}`;
  let since = 0;
  let last = null;

  for (;;) {
    let update;
    try {
      update = await api(`${statusPath}?since=${since}`);
    } catch (err) {
      // A job that has aged out of the server's window, or a poll that raced a
      // restart. Either way there is no progress to show and no result to open.
      throw new DaemonFallback(`the local server stopped reporting progress (${err.message})`);
    }

    for (const event of update.events || []) {
      last = event;
      handleFetchEvent(event, { tab, onEvent });
      // A `done` or an `error` is the end of the log; the loop stops on the
      // server's `done` flag below rather than on the event, so an event type
      // added later cannot be missed by this loop.
      if (event.type === 'done' || event.type === 'error') break;
    }

    // The server's cursor, not a count of what arrived: the two agree only while
    // nothing has been trimmed off the front of the log, and they stop agreeing
    // on exactly the long fetches where the tab most needs to keep up.
    if (typeof update.next === 'number') since = update.next;

    if (update.done) break;
    /* Short enough that the first phase appears as the network announces it, long
     * enough that an idle loop costs nothing measurable over loopback. */
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  if (!last || last.type !== 'done') {
    throw new DaemonFallback('the local daemon did not finish the fetch');
  }
  return last;
}

/* Turn one server event into a tab update and a caller callback.
 *
 * `type` is checked against known values and every field is coerced before use.
 * The data comes from a loopback server that is not this page, so it gets the
 * same distrust as anything else off the wire. */
function handleFetchEvent(event, { tab, onEvent } = {}) {
  if (!event || typeof event !== 'object') return event;
  if (onEvent) onEvent(event);

  switch (event.type) {
    case 'phase':
      toTab(tab, event);
      break;
    case 'status':
      toTab(tab, { type: 'status', message: String(event.message || '') });
      break;
    case 'chunk':
      toTab(tab, {
        type: 'chunk',
        index: Number(event.index) || 0,
        bytes: Number(event.bytes) || 0,
        chunks: Number(event.chunks) || 0,
        total_bytes: Number(event.total_bytes) || 0,
        retried: event.retried === true,
      });
      break;
    case 'done':
      toTab(tab, event);
      break;
    case 'error':
      toTab(tab, { type: 'error', message: String(event.message || 'the fetch failed') });
      break;
  }
  return event;
}

/* Fetch one Autonomi file over WebRTC, reporting progress as it goes.
 *
 * Both the connect and the open call get an `onProgress`, and that is not
 * redundancy: the phases a user actually watches — connecting, looking up which
 * nodes hold the file — are emitted during `connect()`, while the byte and chunk
 * counts come from the stream we consume afterwards. Subscribing to either one
 * alone leaves a gap: the reader alone is silent for the ~6s a lookup takes, which
 * is exactly when a user decides it is broken.
 *
 * Throws `DownloadCancelled` when the user stops it, so callers can say nothing
 * rather than reporting a failure the user asked for. */
class DownloadCancelled extends Error {
  constructor() {
    super('stopped');
    this.name = 'DownloadCancelled';
  }
}

async function fetchAutonomi(address, { onProgress, onStep, onChunk, signal } = {}) {
  /* Announce the library load ourselves, because it is the longest silent stretch
   * in the entire fetch and it is *not* the SDK's to report.
   *
   * `loadSdk()` is a dynamic `import()` of a package carrying 5.4 MB of WASM, and
   * the SDK's first progress event does not fire until after `getNetworkDefaults`
   * has compiled and instantiated it — which happens inside `connect`, before the
   * reporter is even constructed. So without this line the progress tab sits on
   * its opening sentence for the whole load and the dial, which is most of what
   * the user is waiting for, and a slow connection looks exactly like a hang.
   */
  if (onStep) onStep('Loading the Autonomi library (about 5 MB, once)');
  const { mod, error } = await loadSdk();
  if (error) throw new Error(error);
  if (signal && signal.aborted) throw new DownloadCancelled();

  const progress = (event) => {
    if (event && onProgress) onProgress(event);
  };
  if (onStep) onStep('Looking for the nodes holding this file');

  /* **One** argument, and that is load-bearing.
   *
   * `AutonomiClient.connect(bootstrap, options)` reads a plain object in the first
   * argument as a network profile and takes its settings from a rest-spread of
   * that object — the second argument is only consulted for the string form (one
   * explicit WebRTC multiaddress). Passing `{ onProgress }` first and `{ signal }`
   * second therefore silently *drops the abort signal*: `connect` succeeds, and
   * Stop cannot cancel it. Everything goes in the first argument.
   */
  const client = await mod.AutonomiClient.connect({ onProgress: progress, signal });

  /* **Open and stream, do not `download()`.**
   *
   * `download()` is the wrong call for a progress display, and not because of
   * anything in this project. It waits for the whole file and reports once at the
   * end with `phase: "complete"` and the final byte total — so during the part the
   * user is actually waiting for it says nothing at all. The reference app at
   * try.autonomi.com shows per-chunk progress for exactly this reason: it opens a
   * reader and consumes a stream, so it can count arrivals as they happen.
   *
   * `openFile()` gives us two things `download()` never exposes: `reader.size`,
   * known from the DataMap *before* any chunk moves, and a `stream()` that yields
   * the plaintext a chunk at a time. With both, a live "N chunks · X of Y" is
   * arithmetic rather than a guess.
   *
   * The parts are accumulated because the caller still needs the whole file to
   * hand to the server, so this is the same peak memory as `download()`'s blob —
   * the gain is observability, not a smaller footprint. */
  const reader = await client.openFile(address, { onProgress: progress, signal });

  /* The SDK reports `phase: "complete"` the moment this reader exists, which is
   * before a single byte has moved — see `PHASE_LABELS`. So the line under
   * "Fetching from Autonomi…" is set here, by us, and only now is it true. */
  if (onStep) onStep(PHASE_LABELS.downloading);

  const totalBytes = Number(reader.size) || 0;
  if (!totalBytes) throw new Error('the Autonomi library did not report a file size');

  const parts = [];
  let received = 0;
  let chunks = 0;
  const handle = reader.stream({ chunkSize: READ_CHUNK_BYTES, signal }).getReader();
  for (;;) {
    const { done, value } = await handle.read();
    if (done) break;
    if (!value || !value.byteLength) continue;
    parts.push(value);
    received += value.byteLength;
    chunks += 1;
    if (onChunk) onChunk({ chunks, bytes: received, totalBytes });
  }

  if (received !== totalBytes) {
    throw new Error(
      `the Autonomi library returned ${received} of ${totalBytes} bytes`
    );
  }

  /* Nonfatal seed churn is expected: the SDK retries and, when it succeeds,
   * logs nothing to report. Only a settled failure reaches here. */
  return {
    blob: new Blob(parts, {
      type: reader.contentType || 'application/octet-stream',
    }),
  };
}

/* Wire Open and Save for one Autonomi result row. */
function wireFetch(node, result, statusEl) {
  const wrap = node.querySelector('[data-role="fetch"]');
  const openBtn = node.querySelector('[data-role="open"]');
  const saveBtn = node.querySelector('[data-role="save"]');
  const stopBtn = node.querySelector('[data-role="fetch-stop"]');

  const address = autonomiAddress(result.location || '');
  if (!address) {
    /* Not an Autonomi address, so there is nothing to fetch. Stays hidden. */
    return;
  }
  wrap.hidden = false;

  let controller = null;

  const setBusy = (busy) => {
    openBtn.disabled = busy;
    saveBtn.disabled = busy;
    stopBtn.hidden = !busy;
  };

  /* Share one fetch between the two buttons: picking Open over Save and then
   * changing your mind should reuse the bytes already fetched, not pay for the
   * whole download twice. The Blob stays valid after the AbortController is
   * dropped, so it can outlive the fetch it came from. */
  let cached = null;

  /* Fetch over WebRTC into a Blob, sharing one fetch between the two buttons.
   *
   * `tab` is optional: Open passes the window it is driving so the progress goes
   * there, Save passes nothing and keeps the detail in the row. */
  const obtain = async (statusEl, tab = null) => {
    if (cached) return cached;
    controller = new AbortController();
    setBusy(true);
    setStatus(
      statusEl,
      'Looking for the nodes holding this file\u2026',
      ''
    );
    try {
      const { blob } = await fetchAutonomi(address, {
        onProgress: (event) => reportSdkProgress(tab, statusEl, event),
        // The steps the SDK does not report for itself — chiefly the WASM load.
        onStep: (message) => {
          setStatus(statusEl, message, '');
          toTab(tab, { type: 'phase', message });
        },
        // A count per chunk read, which is the live part of the download.
        onChunk: ({ chunks, bytes, totalBytes }) => {
          /* The same `chunk` channel the ant CLI diagnostics use, so one element
           * renders both transports identically instead of each having its own
           * read-out. `expected_bytes` is ours alone \u2014 the CLI reports distinct
           * retrieved chunks and their bytes, which is deliberately not a fraction
           * of the file because self-encryption padding means it never can be. */
          setStatus(
            statusEl,
            `${chunks} chunk${chunks === 1 ? '' : 's'} received \u00b7 ${formatBytes(
              bytes
            )} of ${formatBytes(totalBytes)}`,
            ''
          );
          toTab(tab, {
            type: 'chunk',
            index: chunks,
            chunks,
            total_bytes: bytes,
            expected_bytes: totalBytes,
            retried: false,
          });
        },
        signal: controller.signal,
      });
      cached = blob;
      return blob;
    } finally {
      setBusy(false);
      controller = null;
    }
  };

  /* Fetch, then hand the bytes to the server to serve on its own origin.
   *
   * `openProgressTab` is the first statement, and both the Open button and a click
   * on the location call this directly from their handler, so the popup has a
   * user gesture behind it. A `window.open` after an await is a popup with no
   * gesture and browsers block it — so the tab has to exist before the download
   * starts, and be pointed at the result once it arrives. It opens *onto* the
   * progress page rather than onto `about:blank`, because a tab that says what
   * it is doing is the entire difference between a slow fetch and a broken link. */
  const openView = async (statusEl) => {
    const tab = openProgressTab();
    if (!tab) {
      setStatus(
        statusEl,
        'Your browser blocked the new tab. Allow pop-ups for this page, then try again.',
        'warn'
      );
    }
    try {
      /* Fetched in the browser when it can be, which is the default and needs
       * nothing installed. `obtain` may decline — no WebRTC, no capability, a
       * failed transfer, or a file too large to hand back to the server — and
       * those are all routing decisions rather than failures, so they fall
       * through to the daemon instead of ending the open. */
      let view;
      let bytes = 0;
      let via = 'browser';
      try {
        const blob = await obtain(statusEl, tab);

        /* Decide before uploading. Both reasons used to be discovered by POSTing
         * the file and being refused: a 64 MB record was fetched successfully and
         * then failed with "request body is larger than the 33554432 byte limit",
         * and an unrenderable type was served `inline` and shown as an empty
         * window. We are already holding the bytes in both cases, so neither needs
         * the round-trip — ask for the name and the verdict, then act. Only the
         * leading prefix goes up, so this stays cheap for a 64 MB record. */
        const plan = await postPlan(result.location, blob.type, blob);
        bytes = blob.size;

        if (!plan.renderable || blob.size > MAX_UPLOAD) {
          /* Both reasons are stated when both apply. A large binary hits both, and
           * naming only the type would leave the size — the thing that actually
           * made this slow — unmentioned. */
          const why = [
            !plan.renderable && 'this file type cannot be shown in a browser',
            blob.size > MAX_UPLOAD &&
              `it is ${formatBytes(blob.size)}, over the ${formatBytes(MAX_UPLOAD)} viewer limit`,
          ]
            .filter(Boolean)
            .join(', and ');
          toTab(tab, { type: 'phase', message: 'Saving the file.' });
          downloadBlob(blob, plan.filename);
          if (tab) tab.close();
          setStatus(statusEl, `Downloaded as ${plan.filename} \u2014 ${why}.`, 'ok');
          return;
        }

        setStatus(statusEl, 'Opening\u2026', '');
        view = await postBlob('/api/view/blob', result.location, blob);
      } catch (err) {
        if (err.name === 'DownloadCancelled') throw err;
        /* The daemon fetches and serves the record itself, so the oversize case
         * that just fell through to a download above can instead be opened —
         * which is the point of having it. The tab keeps showing progress
         * throughout, and reports whichever transport actually ran it. */
        view = await fetchViaDaemon(result.location, {
          tab,
          onEvent: (event) => {
            if (event.type === 'status') {
              setStatus(statusEl, event.message, '');
            }
          },
        });
        via = 'daemon';
        bytes = Number(view.bytes) || 0;

        /* The same decision the browser path makes with `postPlan`, and for the
         * same reason. Without it the tab is pointed at a viewer URL that serves
         * `Content-Disposition: attachment`: the browser downloads the file and
         * leaves the tab sitting there claiming it is opening it. Which is the
         * confusing case the progress page's own wording cannot fix — the page has
         * to be told the file is going to be saved rather than shown.
         *
         * `renderable` is the same `is_browser_renderable` verdict `/api/view/plan`
         * gives the browser path, from the same place in `av-ingest`, so the two
         * transports cannot disagree about whether this file will be shown. */
        if (view.renderable === false) {
          toTab(tab, { type: 'phase', message: 'Saving the file.' });
          downloadUrl(view.url, view.filename);
          if (tab) tab.close();
          setStatus(
            statusEl,
            `Downloaded as ${view.filename} \u2014 this file type cannot be shown in a browser.`,
            'ok'
          );
          return;
        }
      }

      setStatus(
        statusEl,
        `Opened ${view.filename} \u2014 ${formatBytes(bytes)} via your ${via}.`,
        'ok'
      );
      if (tab) {
        /* Sever the opener *before* the peer-supplied document is loaded, and not
         * at the open: see `openProgressTab` for why the ordering is the
         * security property. `replace`, not `assign`, so the progress tab does not
         * sit in history with a dead address behind it. */
        cutOpener(tab);
        tab.location.replace(view.url);
      } else {
        setStatus(
          statusEl,
          'The file was fetched but not opened. Allow pop-ups for this page, then try Open again.',
          'warn'
        );
      }
    } catch (err) {
      /* The tab is kept, and told why. It used to be closed on every error,
       * which is what made a 64 MB file look like a link that "does nothing":
       * the tab vanished and the explanation sat in a row behind it. */
      if (err.name === 'DownloadCancelled') {
        if (tab) tab.close();
        setStatus(statusEl, 'Stopped.', '');
      } else {
        toTab(tab, { type: 'error', message: err.message });
        if (tab) {
          /* The error page is our own origin and reflects nothing, so this is
           * hygiene rather than a fix — but the tab is being navigated away from
           * the progress page for good, and the "cut the opener at the handoff"
           * rule should not have a carve-out that only the happy path happens to
           * satisfy. Keeping it here means no future edit to this page can
           * reintroduce the exposure by accident. */
          cutOpener(tab);
          tab.location.replace(
            '/view/error?code=' + (cached ? 'viewer_failed' : 'fetch_failed')
          );
        }
        setStatus(statusEl, `Could not fetch that file: ${err.message}`, 'bad');
      }
    }
  };

  openBtn.addEventListener('click', () => openView(statusEl));

  /* The location itself opens the file. It is the thing that looks like a link,
   * so leaving it inert made a link-shaped element that underlined on hover and
   * did nothing — the Open button existing elsewhere in the row does not make
   * that discoverable. */
  const loc = node.querySelector('.result-loc');
  if (loc) {
    /* The class selects the pointer cursor in both themes; `role` and
     * `tabindex` are what make this reachable and announced as activatable. All
     * three are set here, with the handlers, so the element can never look
     * clickable and not be. */
    loc.classList.add('is-openable');
    loc.setAttribute('role', 'button');
    loc.setAttribute('tabindex', '0');
    loc.title = 'Open this file from the Autonomi network';
    /* Warm the library while the user is still deciding.
     *
     * The first `loadSdk()` compiles and instantiates 5.4 MB of WASM, and that
     * compile is the single longest stretch of a first fetch — long enough to read
     * as "broken" if it is triggered by the click. Hovering or tabbing onto the
     * location is free intent that arrives before the click, so start it there and
     * the wait overlaps with the user's decision instead of following it.
     *
     * `loadSdk()` caches its promise *including its failure*, so this costs nothing
     * if the click never comes and duplicates nothing if it does — the click awaits
     * the same in-flight promise. A pointerenter is not fired by touch, so the
     * focus case is what covers a phone. */
    const warm = () => {
      loadSdk().catch(() => {});
    };
    loc.addEventListener('pointerenter', warm, { once: true });
    loc.addEventListener('focus', warm, { once: true });
    loc.addEventListener('click', () => openView(statusEl));
    loc.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' || event.key === ' ') {
        /* Space would otherwise scroll the page. */
        event.preventDefault();
        openView(statusEl);
      }
    });
  }

  saveBtn.addEventListener('click', async () => {
    try {
      /* No tab here: Save writes a file rather than opening one, so there is
       * nothing for a progress window to be. The row's own status line carries
       * the detail instead. */
      let blob;
      try {
        blob = await obtain(statusEl);
      } catch (err) {
        if (err.name === 'DownloadCancelled') throw err;
        /* The server can fetch it even when this browser cannot — or when the
         * bytes never made it here at all. It serves them on its own throwaway
         * origin, so the Save is the same download-by-link as before. */
        setStatus(statusEl, 'Fetching via your local daemon\u2026', '');
        const view = await fetchViaDaemon(result.location, {
          onEvent: (event) => {
            if (event.type === 'status') setStatus(statusEl, event.message, '');
          },
        });
        downloadBlob(view.url, view.filename);
        setStatus(statusEl, `Saved as ${view.filename} \u2014 via your daemon.`, 'ok');
        return;
      }

      /* Ask the server for the name rather than using the SDK's: it reports
       * `public-file-<8hex>.bin` for every file, so a save here would put
       * everything on disk under the same name.
       *
       * It used to POST the whole record to `/api/view/blob` purely to read that
       * name back, which meant saving anything over `MAX_BODY` failed — having
       * downloaded it successfully first. The bytes are already here; only the
       * name and the verdict are needed, so only a prefix is asked for. */
      const plan = await postPlan(result.location, blob.type, blob);
      downloadBlob(blob, plan.filename);
      setStatus(statusEl, `Saved as ${plan.filename}.`, 'ok');
    } catch (err) {
      if (err.name === 'DownloadCancelled') {
        setStatus(statusEl, 'Stopped.', '');
      } else {
        setStatus(statusEl, `Could not fetch that file: ${err.message}`, 'bad');
      }
    }
  });

  stopBtn.addEventListener('click', () => {
    if (controller) controller.abort();
  });

  /* Gate the buttons on what this browser can do, and only on that.
   *
   * This used to disable both buttons outright when WebRTC was unavailable,
   * which was true once and stopped being true: the daemon can fetch a record on
   * the browser's behalf, so an Autonomi link is openable with nothing installed.
   * Disabling it here would tell the user a file cannot be opened when it can.
   *
   * The title still says *why* the browser is not the transport, because the
   * daemon fetch is slower and it is useful to know which one is about to run.
   * It is set only when this browser genuinely cannot, so it never claims a
   * reason that does not apply. */
  autonomiCapability().then((cap) => {
    if (cap.available) return;
    const hint = `Opening it through your local daemon instead: ${cap.reason}`;
    openBtn.title = hint;
    saveBtn.title = hint;
  });
}

/* --------------------------------------------------------------- search -- */

ui.form.addEventListener('submit', async (event) => {
  event.preventDefault();
  const query = ui.q.value.trim();
  if (!query) return;
  lastQuery = query;

  ui.results.replaceChildren();
  ui.resultsSection.hidden = false;
  ui.resultsEmpty.hidden = true;
  setCount('Searching\u2026');

  try {
    const data = await api(
      `/api/search?q=${encodeURIComponent(query)}&limit=10`
    );
    renderResults(data.results || []);
  } catch (err) {
    ui.results.replaceChildren();
    ui.resultsEmpty.hidden = false;
    ui.resultsEmpty.textContent = `Search failed: ${err.message}`;
    setCount('');
  }
});

function setCount(text) {
  ui.resultsCount.textContent = text;
}

/* Say what just happened, outside the result rows.
 *
 * Removing or blocking a result takes its whole row, and the status line that
 * would have reported the outcome lives inside that row. Reporting it here is
 * the only way the user finds out whether the click did anything. */
function setResultsNote(message, tone) {
  ui.resultsNote.textContent = message || '';
  ui.resultsNote.dataset.tone = tone || '';
}

/* Recount what is left on the page after rows have been taken off it. */
function recountResults() {
  const left = ui.results.children.length;
  ui.resultsEmpty.hidden = left > 0;
  setCount(
    left === 0
      ? `Nothing left for \u201c${lastQuery}\u201d`
      : `${left} result${left === 1 ? '' : 's'} for \u201c${lastQuery}\u201d`
  );
}

/* Take a result off the page and keep the count honest, without re-running the
 * search.
 *
 * Re-querying was the honest way to show what a *fresh* search returns, but it
 * is the wrong tool here: the user just asked for this link to be gone, and
 * asking the network again is both slower and a second chance for a peer to
 * hand back the thing that was just removed. The row is the truth we already
 * have. */
function dropResult(node, message, tone) {
  node.remove();
  recountResults();
  setResultsNote(message, tone);
}

/* Drop every result this page is showing that came from `agent`.
 *
 * Blocking a peer is a statement about the peer, not about one link, so every
 * row it supplied goes at once. Rows that are also local results survive,
 * because a block does not delete the local copy and the user still has it. */
function dropResultsFromAgent(agent) {
  let removed = 0;
  for (const node of Array.from(ui.results.children)) {
    const agents = node.dataset.agentIds ? node.dataset.agentIds.split(',') : [];
    if (!agents.includes(agent)) continue;
    if (node.dataset.source === 'local') continue;
    node.remove();
    removed += 1;
  }
  if (removed === 0) return;
  recountResults();
}

function renderResults(results) {
  ui.results.replaceChildren();
  ui.resultsEmpty.hidden = results.length > 0;
  setResultsNote('', '');

  if (results.length === 0) {
    setCount(`No results for \u201c${lastQuery}\u201d`);
    return;
  }
  setCount(`${results.length} result${results.length === 1 ? '' : 's'} for \u201c${lastQuery}\u201d`);

  for (const result of results) {
    ui.results.appendChild(renderResult(result));
  }
}

function renderResult(result) {
  const node = ui.template.content.firstElementChild.cloneNode(true);
  const pick = (role) => node.querySelector(`[data-role="${role}"]`);

  const source = result.source || 'local';
  const badge = pick('source');
  badge.textContent = source;
  badge.dataset.source = source;

  /* On the row as well as the badge, so a later action can decide what this
   * result is without re-reading the badge, and so `dropResultsFromAgent` can
   * tell a network row from a local one. */
  node.dataset.source = source;
  node.dataset.agentIds = (Array.isArray(result.agent_ids) ? result.agent_ids : [])
    .filter(Boolean)
    .join(',');

  /* Match the CLI's information order: identifier as the heading, description
   * indented underneath.
   *
   * This used to branch on the location's scheme rather than on the result's
   * meaning, so the two layouts disagreed: an `http(s)` result used the
   * *description* as its link text and never displayed the URL at all, while an
   * Autonomi result showed the location with the description below. `av search`
   * has always printed the location first for both, so the browser does too —
   * one rule, one layout.
   *
   * A name result is the one case that inverts, and for the same reason the CLI
   * inverts it: the name is the human-facing identifier and the location is the
   * target it resolves to ("name → target"). Swapping them here would hide the
   * thing the user actually searched for. */
  const location = result.location || '';
  const heading = source === 'name' ? result.description || location : location;
  const detail = source === 'name' ? location : result.description || '';

  /* An `http(s)` location is a plain link. An Autonomi location has no handler
   * registered for its scheme, so it is never an `autonomi://` href — but it is
   * openable, so it keeps the affordances of the link it looks like: app.js
   * wires a click on it below and it takes a pointer cursor. Rendering it as
   * inert text (which is what it was) read as broken: it underlined on hover,
   * showed a question-mark cursor, and did nothing when clicked. */
  const title = pick('title');
  if (/^https?:\/\//i.test(location)) {
    title.textContent = heading;
    title.href = location;
  } else {
    const span = document.createElement('span');
    span.className = 'result-loc';
    span.textContent = heading;
    /* The openable case is completed by wireFetch below, which is where the
     * click handler is added — role, tabindex, title and cursor all live with
     * the handler so a focusable "button" can never exist without one. */
    if (!autonomiAddress(location)) {
      span.title = 'Not a location this page can open.';
    }
    title.replaceWith(span);
    node.classList.add('is-locatable');
  }

  /* The stored name, beside the identifier rather than instead of it. An
   * Autonomi record's canonical location is the address alone, so the
   * `/welcome.md` the author published is not in `location`; without this the
   * row showed a bare address and the name was visible nowhere until the file
   * was saved. Absent for peers, which do not send one — so it stays hidden
   * rather than showing an empty gap. */
  const nameEl = pick('name');
  const storedName = result.filename || '';
  nameEl.textContent = storedName;
  nameEl.hidden = !storedName;

  pick('desc').textContent = detail && detail !== heading ? detail : '';

  const breakdown = result.score_breakdown;
  const score = Number(result.score || 0);
  const scoreEl = pick('score');
  scoreEl.textContent = `score ${score.toFixed(3)}`;
  if (breakdown) {
    /* Tooltip shows why the score is what it is, rather than a vague number. */
    scoreEl.title = Object.entries(breakdown)
      .map(([k, v]) => `${k}: ${Number(v).toFixed(3)}`)
      .join('\n');
  }

  pick('rid').textContent = `id ${(result.resource_id || '').slice(0, 16)}`;
  pick('mime').textContent = result.mime_type || '';

  wireRecommend(node, result, pick('rate-status'));
  wireBlock(node, result, pick('rate-status'), pick('block'));
  wireRemove(node, result, pick('rate-status'), pick('remove'));
  /* Its own status line: a fetch is concurrent with rating/blocking, and
   * sharing a line would mean one overwrote the other's message. */
  wireFetch(node, result, pick('fetch-status'));
  return node;
}

/* -------------------------------------------------------------- blocking -- */

/* Work out what "block" should mean for this result.
 *
 * A name result blocks the name record, not whatever it points at: the
 * complaint is about the claim, and the target may be perfectly good under a
 * different name. Everything else blocks the resource id, which is a content
 * hash, so the block survives the URL moving. */
function blockTarget(result) {
  if (result.source === 'name') {
    const id = result.name_record && result.name_record.record_id;
    return id ? { subject_kind: 'name', subject_id: id } : null;
  }
  return result.resource_id
    ? { subject_kind: 'resource', subject_id: result.resource_id }
    : null;
}

function wireBlock(node, result, statusEl, button) {
  if (!button) return;
  const target = blockTarget(result);

  if (!target) {
    button.disabled = true;
    button.title = 'This result has no id to block.';
    return;
  }

  /* A network result also knows which peers supplied it, so offer blocking
   * those too. That is the only way to act on "this peer keeps sending me
   * junk" without waiting to accumulate a pattern. */
  const agents = Array.isArray(result.agent_ids) ? result.agent_ids.filter(Boolean) : [];

  if (agents.length > 0) {
    const menu = document.createElement('span');
    menu.className = 'block-agents';
    for (const agent of agents) {
      const agentBtn = document.createElement('button');
      agentBtn.type = 'button';
      agentBtn.className = 'btn btn-quiet';
      agentBtn.textContent = `Block peer ${agent.slice(0, 8)}`;
      agentBtn.title = `Stop using answers from ${agent} on this machine`;
      agentBtn.addEventListener('click', async () => {
        /* Blocking a peer is a judgement about one node's behaviour, so the
         * button that makes it should say what it does and, more importantly,
         * what it does not: a two-word label cannot convey that this stops
         * consulting the peer without disconnecting it, and without deleting
         * anything already on this machine. */
        if (!(await askPeerBlock(agent))) return; // cancelled

        agentBtn.disabled = true;
        setStatus(statusEl, 'Blocking peer\u2026', '');
        try {
          await postJSON('/api/block', {
            subject_kind: 'agent',
            subject_id: agent,
            reason: `peer of a search result for \u201c${lastQuery}\u201d`,
          });
          /* A peer block drops its answers, so the results it supplied are no
           * longer what this machine would return. Take them off the page
           * rather than re-asking the network, which is the step that could
           * hand the same answers back. */
          dropResultsFromAgent(agent);
          setResultsNote(
            `Blocked peer ${agent.slice(0, 8)}. Its answers were removed from these results. ` +
              'It can still reach this node \u2014 use "av block --agent ' +
              `${agent} --x0x" to drop it at the transport too.`,
            'ok'
          );
        } catch (err) {
          setStatus(statusEl, err.message, 'bad');
          agentBtn.disabled = false;
        }
      });
      menu.append(agentBtn);
    }
    node.querySelector('.result-actions').append(menu);
  }

  button.addEventListener('click', async () => {
    /* "Block" and "block and delete" are different promises, and the second
     * one cannot be undone by `av unblock`. Ask, with the destructive option
     * named as such, rather than making a single click irreversibly delete
     * rating history. */
    const keepLocal = await askBlock(target);
    if (keepLocal === null) return; // cancelled

    button.disabled = true;
    setStatus(statusEl, 'Blocking\u2026', '');

    try {
      const data = await postJSON('/api/block', {
        ...target,
        keep_local: keepLocal,
      });

      if (!data.blocked) {
        setStatus(statusEl, 'Already blocked \u2014 nothing changed.', 'warn');
        return;
      }

      /* A blocked resource is never served again, and never reappears from a
       * peer, so the row is not merely stale \u2014 it is wrong. Drop it. The
       * local copy may still exist ("Block, keep local copy"), but this page
       * is showing what a search returns, and this will not be in it. */
      dropResult(
        node,
        data.removed_local
          ? `Blocked and deleted the local copy (${data.cascaded} related row${
              data.cascaded === 1 ? '' : 's'
            } went with it). This cannot be undone by unblocking.`
          : 'Blocked. This result will not be shown or served again on this machine.',
        'ok'
      );
    } catch (err) {
      setStatus(statusEl, err.message, 'bad');
      button.disabled = false;
    }
  });
}

/* Ask whether to keep the local copy. Resolves true to keep, false to delete,
 * null if the user cancelled. */
function askBlock(target) {
  const isName = target.subject_kind === 'name';
  ui.blockTitle.textContent = isName ? 'Block this name' : 'Block this resource';
  ui.blockBlurb.textContent = isName
    ? 'The name will stop resolving here and will not be served to peers. The record itself is ' +
      'kept, so unblocking restores it.'
    : 'The resource will disappear from results and will never be served to peers. ' +
      'Blocking alone keeps the local copy; blocking and deleting also removes the local ' +
      'row, its embedding and its ratings, which unblocking cannot undo.';

  /* A name block never deletes anything, so offering "Block and delete" would
   * be a button that appears to do something irreversible and does not. The
   * backend silently ignores keep_local for names; hide the option instead of
   * lying about it. */
  ui.blockDelete.hidden = isName;
  ui.blockKeep.textContent = isName ? 'Block this name' : 'Block, keep local copy';

  return new Promise((resolve) => {
    const onClose = () => {
      ui.blockDialog.removeEventListener('close', onClose);
      const value = ui.blockDialog.returnValue;
      if (value === 'delete' && target.subject_kind !== 'name') resolve(false);
      else if (value === 'keep') resolve(true);
      else resolve(null); // cancel, or Escape
    };
    ui.blockDialog.addEventListener('close', onClose);
    ui.blockDialog.showModal();
  });
}

function wireRecommend(node, result, statusEl) {
  const source = result.source || 'local';
  const buttons = node.querySelectorAll('.btn-rate');

  /* Feedback rows hang off indexed resources, so a rating about a name record
   * has nothing to attach to. Hide the control rather than leave a button that
   * can only ever report that it does not work. */
  if (source === 'name') {
    for (const button of buttons) button.hidden = true;
    return;
  }

  for (const button of buttons) {
    button.addEventListener('click', async () => {
      const rating = button.dataset.rating;

      /* A network result is somebody else's link. Recommending it is the one
       * action here that gives the link to other people, so it asks first. A
       * local result only changes this machine's ranking, so it does not. */
      if (source === 'network') {
        const agreed = await askRecommend();
        /* No, and Escape, are the same answer: do nothing at all. No request,
         * no disabled buttons, no status line — the user declined, so the page
         * should look untouched. */
        if (!agreed) return;
      }

      for (const sibling of buttons) sibling.disabled = true;
      setStatus(statusEl, 'Recording\u2026', '');

      try {
        const data = await postJSON('/api/rate', {
          resource_id: result.resource_id,
          rating,
          query: lastQuery,
          /* Sent back so the server can reinforce this exact result without
           * re-running the search. */
          result,
        });

        let message = data.relevance_boosted
          ? `Recommended. Ranked higher for \u201c${lastQuery}\u201d.`
          : 'Recommended.';

        /* Surfaced because it is the honest limit: publishing does not yet
         * change how peers rank things. Repeatable, so it can be dismissed. */
        if (data.warning) {
          setDismissibleStatus(
            statusEl,
            message + ` ${data.warning}`,
            data.warning_id,
            data.broadcast ? 'warn' : 'ok'
          );
        } else {
          setStatus(statusEl, message, data.broadcast ? 'warn' : 'ok');
        }
      } catch (err) {
        setStatus(statusEl, err.message, 'bad');
        for (const sibling of buttons) sibling.disabled = false;
      }
    });
  }
}

/* Ask before recommending a link that came from the network. Resolves true only
 * for an explicit yes. */
function askRecommend() {
  return new Promise((resolve) => {
    const onClose = () => {
      ui.recommendDialog.removeEventListener('close', onClose);
      resolve(ui.recommendDialog.returnValue === 'yes');
    };
    ui.recommendDialog.addEventListener('close', onClose);
    ui.recommendDialog.showModal();
  });
}

/* ------------------------------------------------------------- remove -- */

/* "Remove from local index" frees local storage. It writes no block, so the
 * difference from Block is not severity but durability: remove can be undone by
 * the network, block cannot be undone without lifting the block. */
function wireRemove(node, result, statusEl, button) {
  if (!button) return;

  /* Only a local resource is in this machine's index to be removed from. A
   * network result has no local row, and a name result is a claim, not a
   * stored resource. */
  if ((result.source || 'local') !== 'local' || !result.resource_id) {
    button.hidden = true;
    return;
  }

  button.addEventListener('click', async () => {
    if (!(await askRemove(result))) return; // cancelled

    button.disabled = true;
    setStatus(statusEl, 'Removing\u2026', '');

    try {
      const data = await postJSON('/api/purge', { resource_id: result.resource_id });
      const n = data.deleted.resources;
      dropResult(
        node,
        n > 0
          ? 'Removed from your local index. A peer holding this link can still return it \u2014 use Block to stop that.'
          : 'Already gone from your local index.',
        n > 0 ? 'warn' : ''
      );
    } catch (err) {
      setStatus(statusEl, err.message, 'bad');
      button.disabled = false;
    }
  });
}

/* Ask before blocking a peer, spelling out both the effect and its limits.
 * Resolves true only for an explicit confirm. */
function askPeerBlock(agent) {
  /* Kept short on purpose. The earlier version spelled out every limit of an
   * agent block in a separate paragraph, which read like a warning about
   * something going wrong. A block does drop this peer's answers and the names
   * it published; the only caveat worth surfacing is that it is not a
   * disconnect, plus the command that is. */
  ui.peerBlockBlurb.textContent =
    `anta-vista will stop asking peer ${agent.slice(0, 8)} for answers, and will ` +
    'ignore anything it sends back.\n\n' +
    'This does not disconnect it — the peer can still reach this node. To drop ' +
    'it at the transport as well, run:\n\n' +
    `    av block --agent ${agent} --x0x\n\n` +
    'Links you have already indexed stay searchable. You can lift the block ' +
    'with av unblock.';

  return new Promise((resolve) => {
    const onClose = () => {
      ui.peerBlockDialog.removeEventListener('close', onClose);
      resolve(ui.peerBlockDialog.returnValue === 'block');
    };
    ui.peerBlockDialog.addEventListener('close', onClose);
    ui.peerBlockDialog.showModal();
  });
}

/* Name what is being removed before removing it, and say plainly that this is
 * not a block. Resolves true only for an explicit confirm. */
function askRemove(result) {
  /* Name the resource by its location, never by its description.
   *
   * `description` is the text anta-vista embeds, not something written for a
   * person: for a text or PDF document it is up to 800 characters of the file's
   * own content, glued into a sentence like "a text document from notes.txt:
   * <the first 800 characters of the file>". Interpolating that into the
   * confirm produced a wall of prose about a deletion, in the one dialog whose
   * whole job is to be readable at a glance.
   *
   * The location is the real identity of the thing being deleted, and it stays
   * short for Autonomi (a hash) and recognisable for a URL. */
  const what = result.location || 'this result';
  ui.removeTitle.textContent = 'Remove from local index?';
  ui.removeBlurb.textContent =
    `This deletes the following from this machine, along with its embedding ` +
    `and ratings:\n\n    ${what}\n\n` +
    'It cannot be undone here, and a peer holding the same link can return it ' +
    'in a later search. Use Block if you want it gone for good.';

  return new Promise((resolve) => {
    const onClose = () => {
      ui.removeDialog.removeEventListener('close', onClose);
      resolve(ui.removeDialog.returnValue === 'remove');
    };
    ui.removeDialog.addEventListener('close', onClose);
    ui.removeDialog.showModal();
  });
}

function setStatus(node, message, tone) {
  node.textContent = message;
  if (tone) node.dataset.tone = tone;
  else delete node.dataset.tone;
  // Any previous "don't show again" control belongs to an older message.
  for (const stale of node.querySelectorAll('.warn-mute')) stale.remove();
}

/* Show a message that the user can permanently dismiss.
 *
 * Warnings here are true but repetitive (peers do not consume feedback yet), so
 * they are stated once and then respected. The suppression is per-warning-id and
 * stored, so a *different* warning still gets through. */
function setDismissibleStatus(node, message, warningId, tone) {
  setStatus(node, message, tone);
  if (!warningId || mutedWarnings().has(warningId)) return;

  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'warn-mute';
  btn.textContent = "Don't show again";
  btn.addEventListener('click', () => {
    muteWarning(warningId);
    setStatus(node, message, tone);
  });
  node.append(btn);
}

/* ---------------------------------------------------------- add a link -- */

/* Thrown by the browser route to say "the daemon can do this instead", carrying
 * why. It is a routing decision rather than a failure, so it must not reach the
 * user as an error. */
class DaemonFallback extends Error {
  constructor(reason) {
    super(reason);
    this.name = 'DaemonFallback';
  }
}

/* Index an Autonomi location using the browser as the fetcher.
 *
 * Returns null when the location is not an Autonomi address — that is a decline,
 * not an error, and the caller should use the daemon route for it. Throws
 * [`DaemonFallback`] when the browser cannot or should not do the fetch. */
async function indexAutonomiInBrowser(uri, tags) {
  const address = autonomiAddress(uri);
  if (!address) return null;

  const cap = await autonomiCapability();
  if (!cap.available) throw new DaemonFallback(cap.reason);

  setStatus(ui.addStatus, 'Looking for the nodes holding this file\u2026', '');
  let blob;
  try {
    ({ blob } = await fetchAutonomi(address, {
      onProgress: (event) => setStatus(ui.addStatus, progressLine(event), ''),
    }));
  } catch (err) {
    /* The SDK's failures are worth trying the daemon on: a WebRTC path that
     * cannot form a data channel here may work fine through a local daemon, and
     * the daemon path is what worked before any of this existed. */
    throw new DaemonFallback(`the browser could not fetch it (${err.message})`);
  }

  /* Checked before the upload, not after: the server rejects an oversized body
   * rather than truncating it, so the bytes are already here and the size is
   * known. Sending them anyway would waste the transfer to learn nothing. */
  if (blob.size > MAX_UPLOAD) {
    throw new DaemonFallback(
      `it is ${formatBytes(blob.size)}, and this page holds at most ${formatBytes(MAX_UPLOAD)}`
    );
  }

  setStatus(ui.addStatus, 'Indexing\u2026', '');
  return postBlob('/api/index/bytes', uri, blob, tags ? { 'X-Anta-Tags': tags } : {});
}

ui.addForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  const uri = el('uri').value.trim();
  if (!uri) return;

  const tags = el('tags').value.trim();
  ui.addStatus.hidden = false;

  /* Two routes, in order of preference: the browser fetches when the location is
   * an Autonomi address and this browser can do it, and the local daemon does
   * everything else. The status line names the route that ran, because "indexed"
   * meaning two different transports would otherwise be invisible. */
  const routes = [
    { via: 'your browser', start: 'Fetching in your browser\u2026', run: () => indexAutonomiInBrowser(uri, tags) },
    { via: 'the local daemon', start: 'Fetching and indexing\u2026', run: () => postJSON('/api/index', { uri, tags }) },
  ];

  let data = null;
  let usedVia = null;
  let fellBack = null;
  try {
    for (const route of routes) {
      setStatus(ui.addStatus, route.start, '');
      try {
        const result = await route.run();
        if (result === null) continue;
        data = result;
        usedVia = route.via;
        break;
      } catch (err) {
        /* Only the browser route signals a fallback; on the daemon route every
         * error is a real failure the user needs to see. */
        if (!(err instanceof DaemonFallback)) throw err;
        fellBack = err.message;
        setStatus(ui.addStatus, `Using the local daemon: ${fellBack}.`, '');
      }
    }
    if (data === null) throw new Error('no route could index that location');

    if (data.duplicate) {
      setStatus(
        ui.addStatus,
        `Already indexed (same ${data.duplicate_by === 'content' ? 'content' : 'URL'}). Use force to re-index.`,
        'warn'
      );
    } else {
      setStatus(
        ui.addStatus,
        `Added \u201c${data.description || data.location}\u201d (${data.embedding_dim} dimensions) via ${usedVia}.`,
        'ok'
      );
    }
    el('uri').value = '';
    el('tags').value = '';
  } catch (err) {
    /* A missing Autonomi backend is a setup problem, not a crash: offer the
     * fix rather than dumping a stack trace. The server sends the exact
     * command for this platform, so we never invent one here. */
    if (err.code === 'no_download_backend') {
      setStatus(ui.addStatus, 'Autonomi support needs the ant CLI or antd daemon.', 'bad');
      openInstall(
        'ant',
        'Install ant CLI',
        err.message,
        (err.body && err.body.command) || ''
      );
    } else {
      setStatus(ui.addStatus, `Could not add that link: ${err.message}`, 'bad');
    }
  }
});

/* --------------------------------------------------------------- status -- */

/* The drawer's visibility is a two-way control, so the trigger is a toggle
 * rather than an "open" button: pressing it again puts the panel away. The
 * label follows the state so it reads correctly without the panel open, and
 * `aria-expanded` carries it for assistive tech. */
function setStatusOpen(open) {
  ui.statusDrawer.hidden = !open;
  ui.overlay.hidden = !open;
  ui.statusBtn.setAttribute('aria-expanded', open ? 'true' : 'false');
  ui.statusBtn.textContent = open ? 'Hide status' : 'Status';
  if (open) refreshStatus();
}

function closeStatus() {
  setStatusOpen(false);
}

ui.statusBtn.addEventListener('click', () => setStatusOpen(ui.statusDrawer.hidden));
ui.closeStatus.addEventListener('click', closeStatus);
ui.overlay.addEventListener('click', closeStatus);

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !ui.statusDrawer.hidden) closeStatus();
});

async function refreshStatus() {
  ui.statusBody.textContent = 'Loading\u2026';
  try {
    const s = await api('/api/status');
    // Blocks are fetched separately because a failed block list must not blank
    // the whole status panel, and vice versa.
    let blocks = null;
    try {
      blocks = await api('/api/blocks');
    } catch (e) {
      blocks = { ok: false, count: 0, blocks: [] };
    }
    ui.statusBody.replaceChildren(statusList(s), blocksSection(blocks));
  } catch (err) {
    ui.statusBody.textContent = `Could not read status: ${err.message}`;
  }
}

/* The blocks list lives in the status drawer so a block can always be undone
 * from the browser. A block the user cannot see and cannot lift from the page
 * they set it on is a trap, so this lists every subject with its own Unblock
 * button rather than only reporting counts. */
function blocksSection(data) {
  const wrap = document.createElement('section');
  wrap.className = 'status-blocks';

  const heading = document.createElement('h3');
  const n = (data && data.count) || 0;
  heading.textContent = `Blocked (${n})`;
  wrap.append(heading);

  const entries = (data && data.blocks) || [];
  if (entries.length === 0) {
    const empty = document.createElement('p');
    empty.className = 'hint';
    empty.textContent = 'Nothing is blocked.';
    wrap.append(empty);
    return wrap;
  }

  const list = document.createElement('ul');
  list.className = 'block-list';
  for (const entry of entries) {
    const li = document.createElement('li');
    li.className = 'block-item';

    const kind = document.createElement('span');
    kind.className = 'badge';
    kind.dataset.source = entry.subject_kind;
    kind.textContent = entry.subject_kind;

    const id = document.createElement('code');
    id.className = 'block-id';
    id.textContent = entry.subject_id;
    if (entry.reason) id.title = entry.reason;

    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'btn btn-quiet';
    btn.textContent = 'Unblock';
    btn.addEventListener('click', async () => {
      btn.disabled = true;
      try {
        const res = await postJSON('/api/unblock', {
          subject_kind: entry.subject_kind,
          subject_id: entry.subject_id,
        });
        setStatus(
          ui.banner,
          res.unblocked
            ? `Unblocked ${entry.subject_kind} ${entry.subject_id.slice(0, 12)}.`
            : 'That subject was not blocked.',
          'ok'
        );
      } catch (err) {
        setStatus(ui.banner, `Could not unblock: ${err.message}`, 'bad');
        btn.disabled = false;
      }
      refreshStatus();
    });

    li.append(kind, id, btn);
    list.append(li);
  }
  wrap.append(list);
  return wrap;
}

function statusList(s) {
  const dl = document.createElement('dl');
  const row = (label, value, tone) => {
    const dt = document.createElement('dt');
    dt.textContent = label;
    const dd = document.createElement('dd');
    dd.textContent = value;
    if (tone) dd.dataset.tone = tone;
    dl.append(dt, dd);
  };

  row('x0x daemon', s.x0x_daemon.running ? `running (${(s.x0x_daemon.agent_id || '').slice(0, 8)})` : 'offline',
      s.x0x_daemon.running ? 'ok' : 'bad');
  row('av listener', s.av_listener.running ? `running (pid ${s.av_listener.pid ?? '?'})` : 'not running',
      s.av_listener.running ? 'ok' : 'warn');
  row('ant daemon', s.ant_daemon.running ? `running (${s.ant_daemon.network})` : 'offline',
      s.ant_daemon.running ? 'ok' : 'warn');
  row('ant CLI', s.ant_cli.installed ? 'installed' : 'not found', s.ant_cli.installed ? 'ok' : 'warn');
  row('MiniLM', s.minilm_model.loaded ? `loaded (${s.minilm_model.dimensions} dimensions)` : 'loads on first search', 'ok');
  row('Resources', String(s.database.resources));
  row('Name records', String(s.database.name_records));
  row('Embeddings', String(s.database.embeddings));
  row('Feedback events', String(s.database.feedback_events));
  row('Uptime', `${s.uptime_seconds}s`);
  return dl;
}

ui.stopListener.addEventListener('click', async () => {
  ui.stopListener.disabled = true;
  try {
    const data = await postJSON('/api/listener/stop', {});
    const n = data.count;
    setStatus(
      ui.statusBody,
      n ? `Stopped ${n} listener process${n === 1 ? '' : 'es'}.` : 'No listener was running.',
      'ok'
    );
    refreshStatus();
  } catch (err) {
    ui.statusBody.textContent = `Could not stop the listener: ${err.message}`;
  } finally {
    ui.stopListener.disabled = false;
  }
});

/* ---------------------------------------------------------- setup banner -- */

async function refreshDeps() {
  let data;
  try {
    data = await api('/api/deps');
  } catch (err) {
    return;
  }
  const d = data.deps || {};

  const missing = [];
  if (!d.x0x_running) {
    missing.push({
      key: 'x0x',
      label: d.x0x_installed ? 'Start x0x' : 'Install x0x',
      start: d.x0x_installed,
      endpoint: '/api/deps/x0x/start',
      command: d.x0x_install_cmd,
      title: 'x0x daemon',
      blurb: d.x0x_installed
        ? 'The x0x binary is installed but the daemon is not answering. Start it to search the network.'
        : 'x0x provides the gossip network Anta-Vista uses to find and share resources.',
    });
  }
  if (!d.av_listener_running) {
    missing.push({
      key: 'listener',
      label: 'Start background listener',
      start: true,
      endpoint: '/api/listener/start',
      command: null,
      title: 'Background listener',
      blurb:
        'The listener answers queries from other agents, so the resources you rate become visible to the network.',
    });
  }

  ui.banner.replaceChildren();
  if (missing.length === 0) {
    ui.banner.hidden = true;
    return;
  }

  const text = document.createElement('span');
  text.textContent =
    missing.length === 1
      ? `Setup needed: ${missing[0].title}.`
      : `Setup needed: ${missing.map((m) => m.title).join(' and ')}.`;
  ui.banner.append(text);

  for (const m of missing) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'btn';
    button.textContent = m.label;
    button.addEventListener('click', () => {
      /* An installed-but-stopped dependency is started, not reinstalled. Only
       * offer the installer when the binary is genuinely missing, so that the
       * user always sees the least destructive action that can work. */
      if (m.start) {
        button.disabled = true;
        button.textContent = 'Starting\u2026';
        postJSON(m.endpoint, {})
          .then((res) => {
            if (res && res.ok === false) {
              button.disabled = false;
              button.textContent = m.label;
              toast(res.detail || 'Could not start ' + m.title + '.', 'err');
              return;
            }
            toast((res && res.detail) || m.title + ' is running.');
            setTimeout(refreshDeps, 1200);
          })
          .catch((err) => {
            button.disabled = false;
            button.textContent = m.label;
            toast(err.message, 'err');
          });
        return;
      }
      openInstall(m.key, m.title, m.blurb, m.command);
    });
    ui.banner.append(button);
  }
  ui.banner.hidden = false;
}

/* ------------------------------------------------------ install dialog -- */

/* Nothing installs without this: the exact command is displayed, and the
 * confirm checkbox is what arms the button. */
function openInstall(key, title, blurb, command) {
  ui.dialogTitle.textContent = title;
  ui.dialogBlurb.textContent = blurb || '';
  ui.dialogCommand.textContent = command || '(no command shown)';
  ui.dialogCommand.hidden = !command;
  ui.dialogConfirm.checked = false;
  ui.dialogRun.disabled = true;
  ui.dialog.dataset.key = key;
  ui.dialog.showModal();
}

ui.dialogConfirm.addEventListener('change', () => {
  ui.dialogRun.disabled = !ui.dialogConfirm.checked;
});

ui.dialog.addEventListener('close', async () => {
  if (ui.dialog.returnValue !== 'run') return;
  const key = ui.dialog.dataset.key;
  const path = key === 'ant' ? '/api/deps/ant/install' : '/api/deps/x0x/install';
  try {
    const data = await postJSON(path, {});
    setStatus(
      ui.banner,
      data.installed ? `${key} installed.` : `${key} install did not complete.`,
      data.installed ? 'ok' : 'bad'
    );
  } catch (err) {
    setStatus(ui.banner, `Install failed: ${err.message}`, 'bad');
  } finally {
    refreshDeps();
  }
});

/* ----------------------------------------------------------------- boot -- */

applyTheme(document.documentElement.dataset.theme || 'retro');
refreshDeps();
setInterval(refreshDeps, 15000);
