/* The Anta-Vista streaming service worker.
 *
 * Two routes, one file, because both stream network bytes to something that
 * wants them chunk by chunk and neither may hold the file in memory:
 *
 * 1. `/av-download/<token>` — a save. The page mints a token per click, posts
 *    the file's name and type (and a MessagePort) to this worker, waits for
 *    the worker's `ready` reply on that port — the token and the download
 *    travel by different routes, so clicking ahead of the registration
 *    races it — then anchors a same-origin navigation at that URL. This
 *    worker answers with a live `ReadableStream` — `Content-Disposition:
 *    attachment` — fed chunk by chunk from the port, and the browser's
 *    download manager spools it straight to disk. Tokens are one-shot and
 *    unguessable (a UUID per click), and a URL carrying a token this worker
 *    does not know is refused *here*, with an empty 404: falling through to
 *    the server would save the error page's bytes as the file, because the
 *    navigation carries the `download` attribute.
 *
 * 2. `/__autonomi_stream/<session>/file` — a playback. The SDK's `MediaBridge`
 *    (via `client.createMediaSource`) registers a session and points a
 *    `<video>`/`<audio>` element at that URL; when the browser asks for a
 *    byte range, this worker posts `autonomi-file-range` to the page and the
 *    bridge answers with the bytes off its random-access reader. The port
 *    protocol for both routes is pull-driven: a slow consumer pauses the
 *    network read rather than queueing the file here.
 *
 * The download half is ours. The media half is upstream's npm
 * `@withautonomi/ant-browser-sdk@0.1.2` `dist/autonomi-stream-sw.js`, merged
 * and **patched**, and the patches are the reason this file exists rather
 * than upstream's:
 *
 *   - Every media response carries `X-Content-Type-Options: nosniff`.
 *   - The Content-Type is allowlisted to `audio/*` and `video/*`. Nothing
 *     else is served, from the URL's `type` parameter or from anywhere else,
 *     so this route — same-origin, as all service-worker routes are — can
 *     never be talked into serving peer-supplied bytes as a document. An
 *     `av-media-type` message lets the page teach the worker the MIME *our*
 *     sniffing decided (`/api/view/plan`), because the SDK reports
 *     `application/octet-stream` for most records and an allowlist that also
 *     admitted octet-stream would either refuse every real file or, worse,
 *     serve media the browser then refuses to play under `nosniff`. A taught
 *     type must itself be audio/* or video/*; anything else is ignored.
 *
 * Served from the app origin with `no-store` (a worker outlives navigations
 * and is only replaced by a registration update — a cached copy would keep
 * running yesterday's code against today's page). */

'use strict';

/* A fresh realm here means the browser (re)started this worker — first
 * registration, a registration update, or a respawn after termination. Two
 * of these lines in one page session is how a mid-transfer death shows up
 * in the console: the download's declared length and the media session's
 * taught MIME both die with the realm that held them. Said to the page too,
 * because a realm restart *is* the idle-kill diagnosis and Firefox shows no
 * worker console: without the post, the second start is invisible exactly
 * where the failure is being investigated. */
function logToClients(label, message) {
  console.log(label, message || '');
  self.clients.matchAll({ type: 'window' }).then((clients) => {
    for (const client of clients) {
      client.postMessage({ type: 'av-stream-log', label, message: message || '' });
    }
  }).catch(() => {});
}
logToClients('av-stream worker started', '');

/* Say what went wrong where the user can see it: Firefox shows no worker
 * console in the page console, so a failure logged only here is invisible —
 * an unknown-token 404 looked like a browser-side download failure with no
 * cause anywhere. Every window client hears it; the page logs it as its
 * own. */
async function announce(label, message) {
  console.error(label, message || '');
  try {
    const clients = await self.clients.matchAll({ type: 'window' });
    for (const client of clients) {
      client.postMessage({ type: 'av-stream-failure', label, message: message || '' });
    }
  } catch (_) {}
}

/* ------------------------------------------------------------------ download */

/* Registered downloads by one-shot token. Bounded by an age sweep on each
 * new registration: a token that was posted to us and never clicked (the
 * anchor click failed, the page died) would otherwise sit here forever. */
const downloads = new Map();

const ENTRY_MAX_AGE_MS = 10 * 60 * 1000;

/* --------------------------------------------------------------------- media */

/* Upstream's constants, kept aligned with its SDK_LIMITS (1 GB media cap,
 * 1 MiB range blocks, 45 s range timeout). */
const STREAM_MARKER = '/__autonomi_stream/';
const STREAM_BLOCK_BYTES = 1024 * 1024;
const RANGE_REQUEST_TIMEOUT_MS = 45000;
const MAX_STREAM_FILE_BYTES = 1000000000;

/* MIMEs the page taught us per media session, overriding the URL's `type`
 * parameter. Bounded by insertion order: a page that starts sessions and
 * never plays them must not grow this forever. */
const mediaTypes = new Map();
const MEDIA_TYPES_MAX = 32;

/* The page that owns each media session — the one whose MediaBridge answers
 * range requests. Recorded from the `av-media-type` teach, whose sender is
 * that page. A playback tab handed the session (the dialog's "Open in
 * another tab") asks *this* client for bytes, not itself: the tab has no
 * bridge of its own, so asking it would time out against a page that cannot
 * answer. Bounded with mediaTypes, evicted in step with it. */
const mediaHosts = new Map();

/* Sessions a page retired mid-playback (`av-media-drop`): its reader is
 * closed, so the next range request would wait out the 45 s range timeout
 * against silence. A retired session 404s at once instead. */
const retiredSessions = new Map();
const RETIRED_MAX = 64;

/* The only Content-Types this worker will ever serve on the media route.
 * Deliberately narrow: same-origin bytes with a document type would be an
 * XSS hole the peer controls. */
function isMediaMime(value) {
  return typeof value === 'string' && /^(audio|video)\//i.test(value);
}

/* A Content-Type that reaches the wire: audio/* or video/* only, capped in
 * length and free of header separators. Anything else — including the
 * SDK's usual `application/octet-stream` — is "no answer", which the media
 * route turns into a 400. */
function mediaContentType(value) {
  if (!isMediaMime(value)) return null;
  if (value.length > 255 || /[\0\r\n]/u.test(value)) return null;
  return value;
}

/* Take over existing pages immediately: both routes are used by the very
 * page that registers us, and waiting for the next navigation to be
 * controlled would strand that first click on the server's 404. */
self.addEventListener('install', () => {
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener('message', (event) => {
  const msg = event.data;
  if (!msg || typeof msg !== 'object') return;
  if (msg.type === 'av-ping') {
    /* Keepalive: there is nothing to compute — dispatching this event is
     * what resets Firefox's ~30-second idle clock on this worker. An idle
     * worker mid-save is a save Firefox kills with the response body and
     * then reports as a file whose source could not be read, so the page
     * pings for as long as anything of ours rides here. */
    return;
  }
  if (msg.type === 'av-download') {
    const port = event.ports && event.ports[0];
    if (!port) return;
    const token = typeof msg.token === 'string' ? msg.token : '';
    if (!token || downloads.has(token)) return;
    const now = Date.now();
    for (const [key, entry] of downloads) {
      if (now - entry.at > ENTRY_MAX_AGE_MS) downloads.delete(key);
    }
    downloads.set(token, {
      port,
      filename: typeof msg.filename === 'string' ? msg.filename : '',
      mime: typeof msg.mime === 'string' ? msg.mime : '',
      size: Number.isSafeInteger(msg.size) && msg.size >= 0 ? msg.size : -1,
      source: typeof msg.source === 'string' ? msg.source : '',
      at: now,
    });
    /* Confirm on the port itself: the page waits for this before it anchors
     * the download, because the token (a message) and the fetch (a
     * navigation) travel by routes whose arrival order is not guaranteed —
     * a fetch that wins gets the 404 below, a failed download, and a page
     * pipe waiting on pulls that will never come. */
    try {
      port.postMessage({ type: 'ready' });
    } catch (_) {}
    return;
  }
  if (msg.type === 'av-media-type') {
    /* The page teaches us the MIME its own sniffing decided, before the
     * player's first range request. Acknowledged on the reply port so the
     * page can await it and only then assign `video.src`. */
    const port = event.ports && event.ports[0];
    const reply = (ok, error) => {
      if (!port) return;
      try {
        port.postMessage(ok ? { ok: true } : { ok: false, error: error || '' });
      } catch (_) {}
      try {
        port.close();
      } catch (_) {}
    };
    const sessionId = typeof msg.sessionId === 'string' ? msg.sessionId : '';
    const mime = mediaContentType(msg.mime);
    if (!/^[0-9a-f]{32}$/u.test(sessionId) || !mime) {
      reply(false, 'not a session this worker serves');
      return;
    }
    if (!mediaTypes.has(sessionId) && mediaTypes.size >= MEDIA_TYPES_MAX) {
      const oldest = mediaTypes.keys().next().value;
      mediaTypes.delete(oldest);
      mediaHosts.delete(oldest);
    }
    mediaTypes.set(sessionId, mime);
    /* The teach's sender is the page whose reader feeds this session — the
     * host. Recorded so a handed-off player tab's range requests are routed
     * back here rather than at the tab, which has no bridge. */
    if (event.source && event.source.id) {
      mediaHosts.set(sessionId, event.source.id);
    }
    reply(true);
  }
  if (msg.type === 'av-media-drop') {
    /* The page that owns a session is retiring it — the row was blocked or
     * removed, or a replacement landed. Forget the session here so the next
     * range request fails at once with a named reason instead of waiting out
     * the range timeout against a reader that is already closed. */
    const sessionId = typeof msg.sessionId === 'string' ? msg.sessionId : '';
    if (!/^[0-9a-f]{32}$/u.test(sessionId)) return;
    mediaTypes.delete(sessionId);
    mediaHosts.delete(sessionId);
    if (!retiredSessions.has(sessionId) && retiredSessions.size >= RETIRED_MAX) {
      retiredSessions.delete(retiredSessions.keys().next().value);
    }
    retiredSessions.set(sessionId, true);
  }
});

self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);
  if (url.origin !== self.location.origin) return;
  /* The standalone media-player page — the tab the dialog's "Open in another
   * tab" sends playback to. Responding to its navigation — with the request
   * itself, byte for byte — is what lands the new document as *our* client:
   * an uncontrolled page's media URL falls through to the server's 404, and
   * the player would find nothing to play. Nothing else about the page comes
   * near here; its script and assets pass to the network below as usual, Host
   * guard included. */
  if (url.pathname === '/media-view') {
    event.respondWith(fetch(event.request));
    return;
  }
  if (url.pathname.startsWith('/av-download/')) {
    if (event.request.method !== 'GET') return;
    const token = url.pathname.slice('/av-download/'.length);
    const entry = downloads.get(token);
    if (!entry) {
      /* A token we never issued, or one already spent. Refusing here rather
       * than letting the request fall through matters: a `download`-attribute
       * navigation that reaches the server's 404 saves the error body as the
       * file, and that is not a download anyone asked for. Announced loudly:
       * this 404 shows the user a browser-side download failure whose only
       * real cause lives in a worker console Firefox does not display. */
      announce(
        'av-stream: unknown download token (the fetch raced the av-download message, or the token was spent)',
        url.pathname
      );
      event.respondWith(new Response(null, { status: 404 }));
      return;
    }
    downloads.delete(token);
    event.respondWith(streamDownload(entry));
    return;
  }
  if (url.pathname.includes(STREAM_MARKER)) {
    event.respondWith(handleStreamRequest(event, url));
  }
  /* Everything else is left to the network — the app's own assets and API. */
});

/* ------------------------------------------------------------------ download */

/* A `Content-Disposition` filename is a header value and an attacker-influenced
 * one at that — it comes from the record's own name. Strip what cannot appear
 * in a quoted-string and fall back rather than refuse the download. */
function dispositionFilename(name) {
  const cleaned = String(name || '').replace(/[\x00-\x1f\x7f"\\]/g, '');
  return cleaned || 'download';
}

function streamDownload({ port, filename, mime, size, source }) {
  /* The pending pull's resolver: one at a time, because the stream machinery
   * only asks again after the previous pull's promise settles. Resolving it
   * is what tells the page it may send the next chunk — and, at the end,
   * that the close it reported has been honoured. */
  let notifyPull = null;
  /* How far this download got, for the cancel report: the browser's own
   * error says only that the source could not be read, and "cancelled at 0
   * bytes" versus "cancelled at 40 MB" are different bugs. */
  const startedAt = Date.now();
  let delivered = 0;

  const stream = new ReadableStream({
    start(controller) {
      port.onmessage = (event) => {
        const msg = event.data;
        if (!msg || typeof msg !== 'object') return;
        if (msg.type === 'chunk' && msg.bytes) {
          try {
            controller.enqueue(msg.bytes);
          } catch (_) {
            /* The consumer is gone; `cancel` already ran or soon will. */
            return;
          }
          delivered += msg.bytes.byteLength;
          if (notifyPull) {
            const n = notifyPull;
            notifyPull = null;
            n();
          }
        } else if (msg.type === 'done') {
          try {
            controller.close();
          } catch (_) {}
          if (notifyPull) {
            const n = notifyPull;
            notifyPull = null;
            n();
          }
        } else if (msg.type === 'error') {
          /* An errored download shows as failed in the browser's download
           * UI — which is the honest outcome: the file did not arrive. The
           * reason is announced (not just logged here) because the download
           * UI shows none of it and Firefox shows no worker console at all. */
          announce(
            'av-stream: download failed',
            msg.message || 'the download failed'
          );
          try {
            controller.error(new Error(msg.message || 'the download failed'));
          } catch (_) {}
        }
      };
    },
    pull() {
      /* Backpressure, one chunk per pull: the promise settles only when the
       * page's answer is enqueued (or the stream ends), so the page cannot
       * outrun the disk. */
      return new Promise((resolve) => {
        notifyPull = resolve;
        try {
          port.postMessage({ type: 'pull' });
        } catch (_) {
          /* The page is gone; stop demanding chunks of a file nobody is
           * feeding. The stream will error on the next consumer action. */
          resolve();
        }
      });
    },
    cancel() {
      /* The user dropped the download in the browser UI — or Firefox tore
       * the channel down for a reason only it knows, which is exactly what
       * needs a name: the page answers this message in silence (a stop is
       * not a failure), and without this line a download dropped mid-file
       * left no trace in any console Firefox shows. Say who asked for it
       * (`source`: which flow started the download) and how far it got. */
      const pullWasOutstanding = notifyPull !== null;
      notifyPull = null;
      announce(
        'av-stream: the browser cancelled this download',
        `${source ? `[${source}] ` : ''}${filename}: ${delivered} bytes delivered after ${Date.now() - startedAt}ms${pullWasOutstanding ? ', a pull was still outstanding' : ''}`
      );
      try {
        port.postMessage({ type: 'cancelled' });
      } catch (_) {}
      try {
        port.close();
      } catch (_) {}
    },
  });

  const headers = {
    'Content-Type': mime || 'application/octet-stream',
    'Content-Disposition':
      'attachment; filename="' + dispositionFilename(filename) + '"',
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
  };
  /* The page knows the size before the first chunk moves (the DataMap says
   * so), and a declared length is what gives the browser's download UI a
   * total to show — and a boundary to fail loudly against if the body stops
   * short. -1 is "unknown": no header, still a valid response. */
  if (size >= 0) headers['Content-Length'] = String(size);

  return new Response(stream, { headers });
}

/* --------------------------------------------------------------------- media */
/* The media route below is upstream's (`dist/autonomi-stream-sw.js`, npm
 * @withautonomi/ant-browser-sdk@0.1.2), patched as the header comment
 * describes: `nosniff` on every response, and a Content-Type allowlist of
 * audio/* and video/* fed by the page's `av-media-type` teaching, with the
 * URL's `type` parameter as the fallback — never as a way in for a
 * non-media type. */

async function handleStreamRequest(event, url) {
  const sessionId = decodeURIComponent(
    (url.pathname.split(STREAM_MARKER, 2)[1] || '').split('/', 1)[0] || ''
  );
  const size = Number(url.searchParams.get('size'));
  const requestedType = url.searchParams.get('type') || '';
  /* Patched: audio/* or video/* only — the page's taught type first (it is
   * the MIME our sniffing decided), then the URL's, then nothing at all. */
  const contentType =
    mediaContentType(mediaTypes.get(sessionId)) ||
    mediaContentType(requestedType);
  const name = url.searchParams.get('name') || 'autonomi-file';
  if (
    !/^[0-9a-f]{32}$/u.test(sessionId) ||
    !Number.isSafeInteger(size) ||
    size <= 0 ||
    size > MAX_STREAM_FILE_BYTES
  ) {
    return new Response('Invalid Autonomi stream URL', { status: 400 });
  }
  if (retiredSessions.has(sessionId)) {
    /* The page retired this session (`av-media-drop`): its reader is gone,
     * so there are no more bytes to ask for. Fail at once rather than
     * starting a stream whose first pull times out. */
    return new Response('This media session has been retired', { status: 404 });
  }
  if (!contentType) {
    /* Patched: this route serves audio and video, and nothing else — a
     * document type here would be same-origin bytes a peer controls. */
    return new Response('This route serves audio and video only', {
      status: 400,
      headers: { 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' },
    });
  }

  const parsed = parseRange(event.request.headers.get('range'), size);
  if (!parsed) {
    return new Response(null, {
      status: 416,
      headers: {
        'Accept-Ranges': 'bytes',
        'Content-Range': 'bytes */' + size,
        'Cache-Control': 'no-store',
        'X-Content-Type-Options': 'nosniff',
      },
    });
  }
  const { start, end, partial } = parsed;
  const responseLength = end - start + 1;
  const headers = {
    'Accept-Ranges': 'bytes',
    'Cache-Control': 'no-store',
    'Content-Disposition':
      "inline; filename*=UTF-8''" + encodeURIComponent(name),
    'Content-Length': String(responseLength),
    'Content-Type': contentType,
    /* Patched: on every media response, not just the errors. */
    'X-Content-Type-Options': 'nosniff',
  };
  if (partial) headers['Content-Range'] = 'bytes ' + start + '-' + end + '/' + size;
  if (event.request.method === 'HEAD') {
    return new Response(null, { status: partial ? 206 : 200, headers });
  }
  if (event.request.method !== 'GET') {
    return new Response('Method not allowed', { status: 405 });
  }

  let offset = start;
  let cancelled = false;
  const body = new ReadableStream({
    async pull(controller) {
      if (offset > end) {
        controller.close();
        return;
      }
      const length = Math.min(STREAM_BLOCK_BYTES, end - offset + 1);
      try {
        const bytes = await requestRangeFromPage(
          event.clientId,
          sessionId,
          offset,
          length
        );
        /* The consumer cancels on every seek — a new range request — and on
         * teardown. The range we were mid-fetching for it is then worthless,
         * and enqueuing into the cancelled stream throws: that TypeError is
         * Chrome's `Cannot enqueue a chunk into a closed readable stream`,
         * console noise that looks like a fault and is not one. */
        if (cancelled) return;
        if (bytes.byteLength !== length) {
          throw new Error(
            'range reader returned ' +
              bytes.byteLength +
              ' bytes, expected ' +
              length
          );
        }
        controller.enqueue(bytes);
        offset += bytes.byteLength;
      } catch (error) {
        if (cancelled) return;
        /* Firefox's own line for this is the generic "a ServiceWorker
         * intercepted the request" one — no route, no reason. The reason
         * (45 s timeout, no page to ask, a refused or short answer) is the
         * only thing that identifies which of those happened. */
        announce('av-stream: media range request failed', String(error));
        try {
          controller.error(error);
        } catch (_) {}
      }
    },
    cancel() {
      cancelled = true;
    },
  });
  return new Response(body, { status: partial ? 206 : 200, headers });
}

function parseRange(header, size) {
  if (!header) return { start: 0, end: size - 1, partial: false };
  const match = /^bytes=(\d*)-(\d*)$/u.exec(header.trim());
  if (!match || (!match[1] && !match[2])) return undefined;
  let start;
  let end;
  if (!match[1]) {
    const suffixLength = Number(match[2]);
    if (!Number.isSafeInteger(suffixLength) || suffixLength <= 0) return undefined;
    start = Math.max(0, size - suffixLength);
    end = size - 1;
  } else {
    start = Number(match[1]);
    end = match[2] ? Number(match[2]) : size - 1;
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end)) return undefined;
    if (start >= size || end < start) return undefined;
    end = Math.min(end, size - 1);
  }
  return { start, end, partial: true };
}

async function requestRangeFromPage(clientId, sessionId, start, length) {
  let client;
  const hostId = mediaHosts.get(sessionId);
  if (hostId) {
    /* A handed-off session: the bytes come from the host page that owns the
     * reader, not from whoever is asking. If that page is gone — closed,
     * crashed, navigated — there is nothing to ask: say so at once, and say
     * it *to the player tab*, whose own error handler cannot tell a dead
     * host from any other stream failure and whose user will ask exactly
     * that question. */
    client = await self.clients.get(hostId);
    if (!client) {
      if (clientId) {
        try {
          const requester = await self.clients.get(clientId);
          if (requester) {
            requester.postMessage({ type: 'av-media-host-gone', sessionId });
          }
        } catch (_) {}
      }
      throw new Error('The Anta-Vista tab that was feeding this media has closed');
    }
  } else {
    client = clientId ? await self.clients.get(clientId) : undefined;
    if (!client) {
      const candidates = await self.clients.matchAll({
        type: 'window',
        includeUncontrolled: true,
      });
      client = candidates[0];
    }
  }
  if (!client) throw new Error('No browser page is available to serve the file range');

  return new Promise((resolve, reject) => {
    const channel = new MessageChannel();
    const fail = (error) => {
      clearTimeout(timeout);
      channel.port1.close();
      reject(error);
    };
    const timeout = setTimeout(() => {
      fail(new Error('Autonomi file range request timed out'));
    }, RANGE_REQUEST_TIMEOUT_MS);
    channel.port1.onmessage = (event) => {
      clearTimeout(timeout);
      channel.port1.close();
      if (!event.data || !event.data.ok) {
        reject(
          new Error(
            (event.data && event.data.error) ||
              'Autonomi file range request failed'
          )
        );
        return;
      }
      resolve(new Uint8Array(event.data.bytes));
    };
    channel.port1.onmessageerror = () => {
      fail(new Error('Autonomi file range response could not be decoded'));
    };
    channel.port1.start();
    try {
      client.postMessage(
        { type: 'autonomi-file-range', sessionId, start, length },
        [channel.port2]
      );
    } catch (error) {
      channel.port2.close();
      fail(error);
    }
  });
}
