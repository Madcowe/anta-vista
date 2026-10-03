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

  /* Autonomi locations are not openable from the browser yet. Rather than
   * link to a scheme nothing handles, show the location as text and let the
   * open/download path be filled in once the server can fetch it. */
  const title = pick('title');
  if (/^https?:\/\//i.test(location)) {
    title.textContent = heading;
    title.href = location;
  } else {
    const span = document.createElement('span');
    span.className = 'result-loc';
    span.textContent = heading;
    /* Say why it is not a link, rather than leaving the user to guess. */
    span.title =
      'Autonomi locations cannot be opened or downloaded yet: the server has no ' +
      'way to fetch them without the ant tooling.';
    title.replaceWith(span);
    node.classList.add('is-locatable');
  }

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

applyTheme(document.documentElement.dataset.theme || 'modern');
refreshDeps();
setInterval(refreshDeps, 15000);
