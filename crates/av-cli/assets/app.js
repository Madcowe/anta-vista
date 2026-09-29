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
  dialogRun: el('install-run'),
};

/* The query the current results belong to. Ratings are boosted per exact
 * query, so this has to travel with the result. */
let lastQuery = '';

/* ---------------------------------------------------------------- theme -- */

function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  // The label names what the button will switch *to*, which is how the two
  // themes have always been described.
  ui.themeToggle.textContent =
    theme === 'retro' ? 'I don\u2019t like now' : 'I don\u2019t like the 90\u2019s';
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

function renderResults(results) {
  ui.results.replaceChildren();
  ui.resultsEmpty.hidden = results.length > 0;

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

  /* Autonomi locations are not openable from the browser yet. Rather than
   * link to a scheme nothing handles, show the location as text and let the
   * open/download path be filled in once the server can fetch it. */
  const location = result.location || '';
  const title = pick('title');
  if (/^https?:\/\//i.test(location)) {
    title.textContent = result.description || location;
    title.href = location;
  } else {
    const span = document.createElement('span');
    span.className = 'result-loc';
    span.textContent = location;
    /* Say why it is not a link, rather than leaving the user to guess. */
    span.title =
      'Autonomi locations cannot be opened or downloaded yet: the server has no ' +
      'way to fetch them without the ant tooling.';
    title.replaceWith(span);
    node.classList.add('is-locatable');
  }

  pick('desc').textContent =
    result.description && result.description !== title.textContent
      ? result.description
      : '';

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

  wireRating(node, result, pick('rate-status'));
  wireBlock(node, result, pick('rate-status'), pick('block'));
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
        agentBtn.disabled = true;
        setStatus(statusEl, 'Blocking peer\u2026', '');
        try {
          await postJSON('/api/block', {
            subject_kind: 'agent',
            subject_id: agent,
            reason: `peer of a search result for \u201c${lastQuery}\u201d`,
          });
          /* A peer block drops its answers, so the result list is now stale.
           * Re-running the search is the honest way to show what is left. */
          setStatus(statusEl, `Blocked peer ${agent.slice(0, 8)}. Re-running search\u2026`, 'ok');
          ui.form.requestSubmit();
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
      } else if (data.removed_local) {
        setStatus(
          statusEl,
          `Blocked and removed the local copy (${data.cascaded} related row${
            data.cascaded === 1 ? '' : 's'
          } deleted). Re-running search\u2026`,
          'ok'
        );
      } else {
        setStatus(statusEl, 'Blocked. Re-running search\u2026', 'ok');
      }

      /* A block changes what this machine will show, so the rendered results
       * are stale by definition. Re-run rather than patching the DOM, so the
       * user sees exactly what a fresh search would return. */
      ui.form.requestSubmit();
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

function wireRating(node, result, statusEl) {
  const source = result.source || 'local';

  for (const button of node.querySelectorAll('.btn-rate')) {
    button.addEventListener('click', async () => {
      const rating = button.dataset.rating;

      /* Feedback rows hang off indexed resources, so a rating about a name
       * record has nothing to attach to. Say so instead of failing silently. */
      if (source === 'name') {
        setStatus(statusEl, 'Name records cannot be rated yet.', 'warn');
        return;
      }

      for (const sibling of node.querySelectorAll('.btn-rate')) sibling.disabled = true;
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
          ? `Rated ${rating}. Ranked higher for \u201c${lastQuery}\u201d.`
          : `Rated ${rating}.`;

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
        for (const sibling of node.querySelectorAll('.btn-rate')) sibling.disabled = false;
      }
    });
  }
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

ui.addForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  const uri = el('uri').value.trim();
  if (!uri) return;

  const tags = el('tags').value.trim();
  ui.addStatus.hidden = false;
  setStatus(ui.addStatus, 'Fetching and indexing\u2026', '');

  try {
    const data = await postJSON('/api/index', tags ? { uri, tags } : { uri });
    if (data.duplicate) {
      setStatus(
        ui.addStatus,
        `Already indexed (same ${data.duplicate_by === 'content' ? 'content' : 'URL'}). Use force to re-index.`,
        'warn'
      );
    } else {
      setStatus(
        ui.addStatus,
        `Added \u201c${data.description || data.location}\u201d (${data.embedding_dim} dimensions).`,
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

function openStatus() {
  ui.statusDrawer.hidden = false;
  ui.overlay.hidden = false;
  ui.statusBtn.setAttribute('aria-expanded', 'true');
  refreshStatus();
}

function closeStatus() {
  ui.statusDrawer.hidden = true;
  ui.overlay.hidden = true;
  ui.statusBtn.setAttribute('aria-expanded', 'false');
}

ui.statusBtn.addEventListener('click', openStatus);
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

applyTheme(document.documentElement.dataset.theme || 'modern');
refreshDeps();
setInterval(refreshDeps, 15000);
