//! Server-rendered shell for the web interface.
//!
//! The page itself is deliberately static: every dynamic value comes from the
//! JSON API, so the browser holds no state the server cannot rebuild. The one
//! server-side substitution is `__BOOT__`, which seeds the initial theme before
//! first paint to avoid a flash of the wrong stylesheet.

use std::sync::Arc;

use super::server::{WebResponse, binary_response, text_response};
use super::state::AppState;

pub fn index_page(_state: &Arc<AppState>) -> WebResponse {
    text_response(200, HTML_MIME, &page())
}

const HTML_MIME: &str = "text/html; charset=utf-8";

/// Inline bootstrap that picks the theme before first paint.
///
/// Kept as its own constant and substituted into `PAGE` by simple replacement:
/// the markup is full of braces, which `format!` would try to interpret.
const BOOT_SCRIPT: &str = r#"(function () {
  try {
    var t = localStorage.getItem('anta-vista-theme');
    if (t !== 'retro' && t !== 'modern') t = 'modern';
    document.documentElement.dataset.theme = t;
  } catch (e) {
    // Private mode or storage disabled: fall back to the default.
    document.documentElement.dataset.theme = 'modern';
  }
  // Where the vendored Autonomi SDK is served. app.js is a classic script, so
  // it cannot `import` this statically; it reads it from here instead.
  window.__AV_SDK_BASE__ = "__SDK_BASE__";
})();"#;

const PAGE: &str = r##"<!doctype html>
<html lang="en" data-theme="modern">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Anta-Vista</title>
<link rel="stylesheet" href="/assets/retro.css">
<link rel="stylesheet" href="/assets/modern.css">
<script>__BOOT__</script>
</head>
<body>
<a class="skip" href="#main">Skip to search</a>

<header class="bar">
  <h1 class="wordmark">Anta<span class="dot">&middot;</span>Vista</h1>
  <nav class="bar-actions">
    <button type="button" id="theme-toggle" class="btn btn-quiet" aria-live="polite">
      I don&rsquo;t like the 90&rsquo;s
    </button>
    <button type="button" id="open-status" class="btn btn-quiet" aria-expanded="false">
      Status
    </button>
  </nav>
</header>

<!-- Shown when the network side needs attention. Populated from /api/deps. -->
<div id="setup-banner" class="banner" hidden role="status"></div>

<main id="main">
  <form id="search-form" class="search" role="search">
    <label class="visually-hidden" for="q">Search the index</label>
    <input id="q" name="q" type="search" autocomplete="off" spellcheck="false"
           placeholder="Search the index&hellip;" required>
    <button type="submit" class="btn btn-primary">Search</button>
  </form>

  <section class="panel" aria-labelledby="addlink-heading">
    <h2 id="addlink-heading">Add a Link</h2>
    <p class="hint">Fetches the page, extracts its text, and adds it to your local index.</p>
    <form id="add-form" class="row">
      <label class="visually-hidden" for="uri">URL or autonomi:// location</label>
      <input id="uri" name="uri" type="text" autocomplete="off" spellcheck="false"
             placeholder="https://example.com  or  autonomi://&hellip;" required>
      <label class="visually-hidden" for="tags">Tags</label>
      <input id="tags" name="tags" type="text" autocomplete="off"
             placeholder="tags, comma separated">
      <button type="submit" class="btn">Add</button>
    </form>
    <p id="add-status" class="status-line" role="status" hidden></p>
  </section>

  <section id="results-section" class="results" aria-labelledby="results-heading" hidden>
    <h2 id="results-heading" class="results-heading"><span id="results-count"></span></h2>
    <ol id="results" class="result-list"></ol>
    <p id="results-empty" class="hint" hidden>No results for that query.</p>
    <!-- Removing or blocking a result takes its whole row, including the status
         line that would have said what happened. This lives outside the rows so
         the outcome survives the removal. -->
    <p id="results-note" class="results-note" role="status"></p>
  </section>
</main>

<!-- Status drawer -->
<aside id="status-drawer" class="drawer" hidden aria-labelledby="status-heading">
  <div class="drawer-head">
    <h2 id="status-heading">Status</h2>
    <button type="button" id="close-status" class="btn btn-quiet" aria-label="Close status">&times;</button>
  </div>
  <div id="status-body" class="drawer-body"></div>
  <div class="drawer-foot">
    <button type="button" id="stop-listener" class="btn btn-quiet">Stop background listener</button>
  </div>
</aside>

<div id="overlay" class="overlay" hidden></div>

<!-- Confirmation dialog, used before running any installer. -->
<dialog id="install-dialog" class="dialog">
  <form method="dialog">
    <h2 id="install-title">Install dependency</h2>
    <p id="install-blurb" class="hint"></p>
    <!-- The exact command is shown before consent; nothing runs until the
         user ticks the box and presses the button below. -->
    <pre id="install-command" class="command"></pre>
    <label class="check">
      <input type="checkbox" id="install-confirm">
      <span>Run this command on my machine</span>
    </label>
    <menu>
      <button value="cancel" class="btn btn-quiet">Cancel</button>
      <button value="run" id="install-run" class="btn btn-primary" disabled>Run installer</button>
    </menu>
  </form>
</dialog>

<!-- Blocking is mostly reversible, but "also delete the local copy" is not, so
     the destructive option is a distinct button rather than a yes/no prompt
     where the wrong click silently does the irreversible thing. -->
<dialog id="block-dialog" class="dialog">
  <form method="dialog">
    <h2 id="block-title">Block this result</h2>
    <p id="block-blurb" class="hint"></p>
    <menu>
      <button value="cancel" class="btn btn-quiet">Cancel</button>
      <button value="keep" class="btn" data-role="block-keep">Block, keep local copy</button>
      <!-- A name block only hides the record; there is nothing to delete, so
           this button is hidden for names rather than offering an action the
           backend silently ignores. See askBlock() in app.js. -->
      <button value="delete" class="btn btn-primary" data-role="block-delete">Block and delete</button>
    </menu>
  </form>
</dialog>

<dialog id="recommend-dialog" aria-labelledby="recommend-title">
  <form method="dialog">
    <h2 id="recommend-title">Share this link?</h2>
    <p class="hint">Recommending it may share the link with other people using this network.</p>
    <menu>
      <button value="no" class="btn btn-quiet">No, keep it to myself</button>
      <button value="yes" class="btn btn-primary">Yes, recommend</button>
    </menu>
  </form>
</dialog>

<dialog id="peer-block-dialog" aria-labelledby="peer-block-title">
  <form method="dialog">
    <h2 id="peer-block-title">Block this peer?</h2>
    <!-- Filled in by askPeerBlock() in app.js. An agent block is not a
         disconnect, so the peer being able to reach this node is part of the
         promise rather than a footnote. The command to drop it at the
         transport travels with the copy. -->
    <p id="peer-block-blurb" class="hint"></p>
    <menu>
      <button value="cancel" class="btn btn-quiet">Cancel</button>
      <button value="block" class="btn btn-danger">Block this peer</button>
    </menu>
  </form>
</dialog>

<dialog id="remove-dialog" aria-labelledby="remove-title">
  <form method="dialog">
    <h2 id="remove-title">Remove from local index?</h2>
    <p id="remove-blurb" class="hint"></p>
    <menu>
      <button value="cancel" class="btn btn-quiet">Cancel</button>
      <button value="remove" class="btn btn-danger" data-role="remove-confirm">Remove</button>
    </menu>
  </form>
</dialog>

<template id="result-template">
  <li class="result">
    <div class="result-head">
      <span class="badge" data-role="source"></span>
      <a class="result-title" data-role="title" target="_blank" rel="noopener noreferrer"></a>
    </div>
    <p class="result-desc" data-role="desc"></p>
    <div class="result-meta">
      <span class="score" data-role="score"></span>
      <span class="rid" data-role="rid"></span>
      <span class="mime" data-role="mime"></span>
    </div>
    <div class="result-actions">
      <!-- One rating, not three. The value stays "useful" — only the label
           changed — and the two negative ratings are no longer offered here.
           A name record cannot be rated, so app.js hides the whole control
           for source="name" rather than showing a button that cannot work. -->
      <button type="button" class="btn btn-rate" data-rating="useful"
        title="Recommend this result for the query you searched">Recommend</button>
      <!-- Only offered when the result is local, which app.js decides from
           result.source. Removes the local copy without blocking it: a peer
           holding the same link can return it later. -->
      <span class="result-deny">
        <button type="button" class="btn btn-quiet btn-remove" data-role="remove"
          title="Delete this from your local index. A peer holding it can still return it; use Block to stop that.">Remove from local index</button>
        <button type="button" class="btn btn-quiet btn-block" data-role="block"
          title="Never show or serve this again on this machine">Block</button>
      </span>
      <!-- Only for an Autonomi result, and only once the browser library is
           known to be usable; app.js owns the visibility. "Stop" is hidden
           until a fetch is actually running, because an offer to cancel
           nothing is noise. -->
      <span class="result-fetch" data-role="fetch" hidden>
        <button type="button" class="btn btn-quiet btn-open" data-role="open"
          title="Fetch this from the Autonomi network and open it in a tab">Open</button>
        <button type="button" class="btn btn-quiet btn-save" data-role="save"
          title="Fetch this from the Autonomi network and save it to this device">Save</button>
        <button type="button" class="btn btn-quiet btn-fetch-stop" data-role="fetch-stop" hidden
          title="Stop fetching this file">Stop</button>
      </span>
      <!-- Fetch progress and outcome get their own line rather than reusing
           rate-status: rating, blocking and removing all report there, and a
           fetch that overwrites their message would lose it. -->
      <span class="fetch-status" data-role="fetch-status" role="status"></span>
      <span class="rate-status" data-role="rate-status" role="status"></span>
    </div>
  </li>
</template>

<script src="/assets/app.js"></script>
<script src="/assets/retro.js"></script>
</body>
</html>
"##;

fn page() -> String {
    PAGE.replace("__BOOT__", BOOT_SCRIPT)
        .replace("__SDK_BASE__", SDK_BASE)
}

pub fn retro_css() -> WebResponse {
    text_response(200, CSS_MIME, include_str!("../../assets/retro.css"))
}

pub fn modern_css() -> WebResponse {
    text_response(200, CSS_MIME, include_str!("../../assets/modern.css"))
}

pub fn app_js() -> WebResponse {
    text_response(200, JS_MIME, include_str!("../../assets/app.js"))
}

pub fn retro_js() -> WebResponse {
    text_response(200, JS_MIME, include_str!("../../assets/retro.js"))
}

/// Vendored third-party browser code, served under a version-prefixed path.
///
/// The Autonomi browser SDK is an ES module graph plus a WebAssembly binary that
/// the browser loads lazily, when the user first asks to view or index an
/// Autonomi link. It is vendored rather than pulled from a CDN so the bytes we
/// execute are pinned in the repository and reviewed like any other dependency.
///
/// `include_bytes!` instead of a directory walk: the file set is fixed at
/// compile time, a missing or renamed file is a build error rather than a 404
/// that only shows up when a user clicks a link, and it needs no new dependency
/// to do the embedding.
///
/// Only the files reachable from `index.js` are vendored, minus source maps and
/// type declarations. The set was derived mechanically by walking the import
/// graph -- including the imports *inside* `new Worker(new URL(...))` scripts,
/// which a naive walk misses and which fail only at runtime, inside the worker.
pub fn vendor_asset(rel: &str) -> Option<WebResponse> {
    let (mime, body): (&str, &[u8]) = match rel {
        "capabilities.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/capabilities.js").as_slice(),
        ),
        "client.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/client.js").as_slice(),
        ),
        "errors.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/errors.js").as_slice(),
        ),
        "file-reader.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/file-reader.js").as_slice(),
        ),
        "index.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/index.js").as_slice(),
        ),
        "internal/abort.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/abort.js")
                .as_slice(),
        ),
        "internal/media.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/media.js")
                .as_slice(),
        ),
        "internal/payment-network.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/payment-network.js")
                .as_slice(),
        ),
        "internal/progress.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/progress.js")
                .as_slice(),
        ),
        "internal/protocol.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/protocol.js")
                .as_slice(),
        ),
        "internal/record-store.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/record-store.js")
                .as_slice(),
        ),
        "internal/runtime.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/runtime.js")
                .as_slice(),
        ),
        "internal/snapshot.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/snapshot.js")
                .as_slice(),
        ),
        "internal/staging.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/staging.js")
                .as_slice(),
        ),
        "internal/upload-recovery.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/upload-recovery.js")
                .as_slice(),
        ),
        "internal/upload-sources.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/upload-sources.js")
                .as_slice(),
        ),
        "internal/window-checkpoint.js" => (
            JS_MIME,
            include_bytes!(
                "../../assets/vendor/ant-browser-sdk/0.1.1/internal/window-checkpoint.js"
            )
            .as_slice(),
        ),
        "internal/window-stager.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/internal/window-stager.js")
                .as_slice(),
        ),
        "limits.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/limits.js").as_slice(),
        ),
        "manual-payment.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/manual-payment.js")
                .as_slice(),
        ),
        "network-profile.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/network-profile.js")
                .as_slice(),
        ),
        "payment.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/payment.js").as_slice(),
        ),
        "save.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/save.js").as_slice(),
        ),
        "upload-worker.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/upload-worker.js").as_slice(),
        ),
        "wasm/ant_core.js" => (
            JS_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/wasm/ant_core.js").as_slice(),
        ),
        "wasm/ant_core_bg.wasm" => (
            WASM_MIME,
            include_bytes!("../../assets/vendor/ant-browser-sdk/0.1.1/wasm/ant_core_bg.wasm")
                .as_slice(),
        ),
        _ => return None,
    };
    Some(binary_response(200, mime, body.to_vec()))
}

/// Strip the [`SDK_BASE`] prefix from a request path, yielding the vendored
/// relative path, or `None` if the path is not under it.
pub fn vendor_rel_path(path: &str) -> Option<&str> {
    path.strip_prefix(SDK_BASE)
        .filter(|r| !r.is_empty() && !r.contains(".."))
}

/// Version of the vendored Autonomi browser SDK. Part of the asset path so a
/// future SDK lands under a new prefix and browsers cannot serve a stale mix of
/// two versions' modules against one cached WASM binary.
pub const SDK_VERSION: &str = "0.1.1";

/// Where the vendored SDK is served from, relative to this server's root.
pub const SDK_BASE: &str = "/assets/vendor/autonomi-sdk/0.1.1/";

const CSS_MIME: &str = "text/css; charset=utf-8";
const JS_MIME: &str = "application/javascript; charset=utf-8";

/// Mandatory, not cosmetic: `WebAssembly.instantiateStreaming` refuses any
/// content type other than this, so serving the SDK's WASM as anything else
/// fails at download time with a MIME error that reads like a network problem.
const WASM_MIME: &str = "application/wasm";

/// The page a fetch failure lands in, shown in the tab that was opened for the
/// file.
///
/// It exists because the tab used to just close. A tab that vanishes leaves the
/// explanation behind it, in a row on a page the user has to go back to; a tab
/// that says why keeps the reason where the user is already looking. It is
/// deliberately standalone — no `app.js`, no stylesheet, no API access — and the
/// only input is a code matched against a fixed table below.
///
/// Nothing from the network reaches this page. An SDK error string is attacker-
/// influenced, so it is written into the row's status line with `textContent`
/// and never interpolated into markup here; the codes below are the whole
/// vocabulary.
pub fn view_error_page(query: &str) -> WebResponse {
    let code = query_param(query, "code").unwrap_or_default();
    let (title, detail) = match code.as_str() {
        "fetch_failed" => (
            "That file could not be fetched",
            "The Autonomi network did not deliver this record. It may be offline, \
             or the address may not point at anything fetchable.",
        ),
        "no_backend" => (
            "No way to open this file",
            "This browser cannot reach the Autonomi network, and no local daemon \
             is available to fetch it instead.",
        ),
        "blocked_popup" => (
            "Your browser blocked the tab",
            "Allow pop-ups for this page, then try Open again.",
        ),
        _ => (
            "That file could not be opened",
            "Something went wrong before the file could be shown. The details are \
             in the anta-vista tab this one was opened from.",
        ),
    };

    text_response(200, HTML_MIME, &error_page_html(title, detail))
}

/// Pull one parameter out of a raw query string, percent-decoding it.
///
/// Deliberately minimal and allocation-light: the only caller passes a `code`
/// that is matched against fixed strings, so a malformed escape yields no match
/// rather than an error path of its own.
fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            return Some(v.replace('+', " "));
        }
    }
    None
}

/// Escapes the two characters that matter in text content. `title` and `detail`
/// are compile-time constants today, so this is belt-and-braces for the next
/// caller rather than a live need — but an HTML builder that cannot escape is a
/// trap, and this one is small.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn error_page_html(title: &str, detail: &str) -> String {
    let t = esc(title);
    let d = esc(detail);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>{t}</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{
    margin: 0; min-height: 100vh; display: grid; place-items: center;
    padding: 2rem; box-sizing: border-box;
    font: 16px/1.55 ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
    background: Canvas; color: CanvasText;
  }}
  main {{ max-width: 34rem; }}
  h1 {{ font-size: 1.25rem; margin: 0 0 .5rem; }}
  p {{ margin: 0; opacity: .8; }}
</style>
</head>
<body>
<main>
<h1>{t}</h1>
<p>{d}</p>
</main>
</body>
</html>
"#
    )
}
