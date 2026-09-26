//! Server-rendered shell for the web interface.
//!
//! The page itself is deliberately static: every dynamic value comes from the
//! JSON API, so the browser holds no state the server cannot rebuild. The one
//! server-side substitution is `__BOOT__`, which seeds the initial theme before
//! first paint to avoid a flash of the wrong stylesheet.

use std::sync::Arc;

use super::server::{WebResponse, text_response};
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
      <span class="rate-label">Rate:</span>
      <button type="button" class="btn btn-rate" data-rating="useful" title="Useful: raises this result's score for this query">Useful</button>
      <button type="button" class="btn btn-rate" data-rating="not-useful" title="Not useful: lowers it">Not useful</button>
      <button type="button" class="btn btn-rate" data-rating="incorrect" title="Factually wrong">Incorrect</button>
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

const CSS_MIME: &str = "text/css; charset=utf-8";
const JS_MIME: &str = "application/javascript; charset=utf-8";
