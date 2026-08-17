// ==========================================================================
// Engram Memory Vault — SPA
// ==========================================================================

import { MemoryGraph } from './graph.js';
import * as unlock from './unlock.js';
import { BIP39_WORDS } from './vendor/bip39-english.js';

// Daemon base, resolved per call. On the box console the SPA is served by
// Caddy, which reverse-proxies these paths to the local engramd — relative
// works there. On the public site there is no proxy: the browser talks to
// the user's own loopback daemon (the `engram handoff` tunnel target).
// A localStorage override ('engram-daemon-base') wins when set.
function daemonApiBase() {
  const saved = localStorage.getItem('engram-daemon-base');
  if (saved) return saved;
  if (window.location.hostname === 'localhost' || window.location.hostname === '127.0.0.1' || window.location.hostname === '[::1]') return '';
  if (hasAuth()) return '';  // box console: same-origin via Caddy
  return 'http://127.0.0.1:8799';  // public site: the user's loopback daemon
}

// Chrome 142+ Local Network Access: a public site fetching a loopback
// daemon must declare the target address space, and the daemon's preflight
// must carry Access-Control-Allow-Private-Network:true (origin-gated) —
// both already wired on the daemon for the handoff route.
function daemonFetch(path, opts = {}) {
  const base = daemonApiBase();
  const isLoopback = /^https?:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?/.test(base);
  return fetch(base + path, isLoopback ? { ...opts, targetAddressSpace: 'loopback' } : opts);
}

// ── Theme toggle ───────────────────────────────────────────────────────────

(function initTheme() {
  const saved = localStorage.getItem('engram-theme');
  if (saved === 'light') document.documentElement.setAttribute('data-theme', 'light');
  const btn = document.getElementById('theme-toggle');
  if (btn) {
    btn.textContent = saved === 'light' ? '☾' : '☀';
    btn.addEventListener('click', () => {
      const current = document.documentElement.getAttribute('data-theme');
      if (current === 'light') {
        document.documentElement.removeAttribute('data-theme');
        localStorage.setItem('engram-theme', 'dark');
        btn.textContent = '☀';
      } else {
        document.documentElement.setAttribute('data-theme', 'light');
        localStorage.setItem('engram-theme', 'light');
        btn.textContent = '☾';
      }
    });
  }
})();

// ── API client ────────────────────────────────────────────────────────────

async function get(path) {
  const r = await daemonFetch(path, { headers: authHeaders() });
  if (r.status === 401) onApi401(r);
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  return r.json();
}

async function post(path, body = {}) {
  const r = await daemonFetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders() },
    body: JSON.stringify(body),
  });
  if (r.status === 401) onApi401(r);
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  return r.json();
}

// ── Sync-server (relay) account client ─────────────────────────────────────
// The relay's error shape is {error: {code, error}} — surface the message
// and keep code/status on the Error so the panel can branch on them.

async function acctReq(server, path, opts = {}) {
  const headers = { 'Content-Type': 'application/json', ...(opts.headers || {}) };
  const token = localStorage.getItem('engram-sync-session');
  if (token) headers['Authorization'] = 'Bearer ' + token;
  const r = await fetch(String(server).replace(/\/+$/, '') + path, { ...opts, headers });
  if (!r.ok) {
    let code = null, msg = `${r.status} ${r.statusText}`;
    try {
      const b = await r.json();
      // Account routes use the flat {code, error} shape; pull/device routes
      // nest under `error`. Handle both.
      if (typeof b?.error === 'string') { code = b.code ?? null; msg = b.error; }
      else if (b?.error?.error) { code = b?.error?.code || null; msg = b.error.error; }
    } catch {}
    const e = new Error(msg);
    e.status = r.status;
    e.code = code;
    throw e;
  }
  return r.json();
}
const acctGet = (server, path) => acctReq(server, path);
const acctPost = (server, path, body) => acctReq(server, path, { method: 'POST', body: JSON.stringify(body || {}) });
const acctPut = (server, path, body) => acctReq(server, path, { method: 'PUT', body: JSON.stringify(body || {}) });
const acctDel = (server, path) => acctReq(server, path, { method: 'DELETE' });

const api = {
  health: () => get('/health'),
  stats: () => get('/analytics/stats'),
  memories: {
    search: (q) => post('/memories/search', q),
    get: (id) => get('/memories/' + id),
    create: (m) => post('/memories', m),
    links: (id) => get('/memories/' + id + '/links'),
    related: (id, limit = 10) => get('/memories/' + id + '/related?limit=' + limit),
    ground: (id) => post('/memories/' + id + '/ground'),
    markNoise: (id) => post('/memories/' + id + '/mark-noise'),
    delete: async (id) => {
      const r = await daemonFetch('/memories/' + id, { method: 'DELETE', headers: authHeaders() });
      if (r.status === 401) onApi401(r);
    },
    annotations: (id) => get('/memories/' + id + '/annotations'),
    annotate: (id, content) => post('/memories/' + id + '/annotations', { content }),
  },
  annotations: {
    delete: async (id) => {
      const r = await daemonFetch('/annotations/' + id, { method: 'DELETE', headers: authHeaders() });
      if (r.status === 401) onApi401(r);
    },
  },
  analytics: {
    activity: (days = 30) => get('/analytics/activity?days=' + days),
    co2: () => get('/analytics/co2'),
  },
  savedSearches: {
    list: () => get('/searches'),
    create: (s) => post('/searches', s),
    delete: async (id) => {
      const r = await daemonFetch('/searches/' + id, { method: 'DELETE', headers: authHeaders() });
      if (r.status === 401) onApi401(r);
    },
  },
  context: {
    assemble: (q) => post('/context/assemble', q),
  },
  consolidate: {
    decay: () => post('/consolidate/decay'),
    weekly: () => post('/consolidate/weekly'),
    history: () => get('/consolidate/history'),
  },
  patterns: (q) => post('/analytics/patterns', q),
  export: (q) => post('/export', q || {}),
  import: (body) => post('/import', body),
  privacy: {
    audit: () => get('/privacy/audit'),
    purge: (criteria) => post('/privacy/purge', criteria),
  },
  config: {
    get: () => get('/config'),
    update: async (c) => {
      const r = await daemonFetch('/config', { method: 'PATCH', headers: { 'Content-Type': 'application/json', ...authHeaders() }, body: JSON.stringify(c) });
      if (r.status === 401) onApi401(r);
      return r;  // callers inspect r.ok themselves (sync key save)
    },
  },
  teams: {
    status: () => get('/teams/status'),
  },
  account: {
    // Account endpoints live on the SYNC SERVER (the relay), not the
    // daemon: WebAuthn ceremonies and key management are relay features.
    // The browser calls them directly (the relay's CORS allows it) and
    // the session Bearer token lives in localStorage — cross-site cookies
    // would never stick from localhost:8787 → sync.ellmstack.dev.
    token: () => localStorage.getItem('engram-sync-session'),
    setToken: (t) => t ? localStorage.setItem('engram-sync-session', t) : localStorage.removeItem('engram-sync-session'),
    registerStart: (server, origin) => acctPost(server, '/auth/register/start', { origin }),
    registerFinish: (server, origin, challengeId, registration) =>
      acctPost(server, '/auth/register/finish', { origin, challenge_id: challengeId, registration }),
    loginStart: (server, origin) => acctPost(server, '/auth/login/start', { origin }),
    loginFinish: (server, origin, challengeId, credential) =>
      acctPost(server, '/auth/login/finish', { origin, challenge_id: challengeId, credential }),
    logout: (server) => acctPost(server, '/auth/logout', {}),
    get: (server) => acctGet(server, '/account'),
    createKey: (server, vaultId) => acctPost(server, '/account/keys', vaultId ? { vault_id: vaultId } : {}),
    revokeKey: (server, keyId) => acctDel(server, '/account/keys/' + encodeURIComponent(keyId)),
    pairCodes: (server) => acctPost(server, '/devices/pair-codes', {}),
    // ── Email+password account routes (password_routes.rs) ────────────────
    signup: (server, email, password) => acctPost(server, '/auth/signup', { email, password }),
    signin: (server, email, password) => acctPost(server, '/auth/signin', { email, password }),
    resetRequest: (server, email) => acctPost(server, '/auth/reset/request', { email }),
    resetConfirm: (server, token, newPassword) => acctPost(server, '/auth/reset/confirm', { token, new_password: newPassword }),
    // ── Zero-knowledge envelopes (account_routes.rs) ──────────────────────
    credentials: (server) => acctGet(server, '/account/credentials'),
    changePassword: (server, currentPassword, newPassword) =>
      acctPost(server, '/account/password', { current_password: currentPassword, new_password: newPassword }),
    passkeys: (server) => acctGet(server, '/account/passkeys'),
    // Detaching a passkey is a credential mutation: password accounts must
    // verify their password fresh, in this request (server-enforced).
    detachPasskey: (server, credentialId, password) =>
      acctReq(server, '/account/passkeys/' + encodeURIComponent(credentialId),
        { method: 'DELETE', body: JSON.stringify(password ? { password } : {}) }),
    wraps: (server) => acctGet(server, '/account/wraps'),
    getPasswordWrap: (server) => acctGet(server, '/account/wraps/password'),
    putPasswordWrap: (server, wrappedA, saltPw) =>
      acctPut(server, '/account/wraps/password', { wrapped_a: wrappedA, salt_pw: saltPw }),
    getRecoveryWrap: (server) => acctGet(server, '/account/wraps/recovery'),
    putRecoveryWrap: (server, wrappedARec, saltRec) =>
      acctPut(server, '/account/wraps/recovery', { wrapped_a_rec: wrappedARec, salt_rec: saltRec }),
    getVaultWrap: (server, vaultId) =>
      acctGet(server, '/account/vaults/' + encodeURIComponent(vaultId) + '/wrap'),
    putVaultWrap: (server, vaultId, wrappedK) =>
      acctPut(server, '/account/vaults/' + encodeURIComponent(vaultId) + '/wrap', { wrapped_k: wrappedK }),
    // Locking a vault gates key access: password accounts verify their
    // password fresh, in this request (server-enforced).
    deleteVaultWrap: (server, vaultId, password) =>
      acctReq(server, '/account/vaults/' + encodeURIComponent(vaultId) + '/wrap',
        { method: 'DELETE', body: JSON.stringify(password ? { password } : {}) }),
  },
  sync: {
    now: () => post('/sync/now'),
  },
  digest: {
    // Structured daemon errors (409 digest_disabled / llm_not_configured)
    // carry a readable message — surface it instead of "409 Conflict".
    weekly: async (days = 7, prose = false) => {
      const r = await daemonFetch('/digest/weekly?days=' + days + (prose ? '&prose=1' : ''), { headers: authHeaders() });
      if (r.status === 401) onApi401(r);
      if (!r.ok) {
        let msg = `${r.status} ${r.statusText}`;
        try { const b = await r.json(); if (b?.error?.message) msg = b.error.message; } catch {}
        throw new Error(msg);
      }
      return r.json();
    },
  },
};

// ── Vault gate (HTTP basic auth via the branded login screen) ─────────────
// Caddy gates the API paths with HTTP basic auth but strips the
// WWW-Authenticate challenge (site config), so the browser never shows the
// native popup — this SPA supplies the credentials instead. Form credentials
// live ONLY in sessionStorage (per-tab, dies with the tab): the user chose
// per-session auth, no "keep me signed in", because the vault guards
// sensitive data. base64-in-storage ≈ the trust level of the browser's own
// auth cache, mitigated by the strict CSP (script-src 'self').

const VAULT_CREDS_KEY = 'engram-vault-creds';
// Set only by account registration (never login): the login screen shows
// the Add-device wizard exactly once, right after sign-up — a passkey
// LOGIN lands back on the vault sign-in form, not on the wizard.
const VAULT_JUST_REGISTERED_KEY = 'engram-just-registered';
// Set when a pairing code is minted this visit: the login screen then
// polls the account's vaults and auto-advances to the unlock picker the
// moment the paired device registers on the relay.
const PAIR_POLL_KEY = 'engram-pair-poll';
// A key-handoff link opened without a live session must survive the sign-in
// round-trip: the handoff route stashes the token here, /login resumes it
// the moment a session exists. Single-shot and session-scoped by design.
const HANDOFF_PENDING_KEY = 'engram-pending-handoff';

function pendingHandoff() {
  try {
    const v = JSON.parse(sessionStorage.getItem(HANDOFF_PENDING_KEY) || 'null');
    return v && v.token ? v : null;
  } catch { return null; }
}

function resumePendingHandoff() {
  const p = pendingHandoff();
  if (!p || !localStorage.getItem('engram-sync-session')) return false;
  sessionStorage.removeItem(HANDOFF_PENDING_KEY);
  navigate('#/handoff/' + encodeURIComponent(p.token) + '?daemon=' + encodeURIComponent(p.daemon || '127.0.0.1:8799'));
  return true;
}

function getCreds() {
  return sessionStorage.getItem(VAULT_CREDS_KEY);
}

function setCreds(b64) {
  sessionStorage.setItem(VAULT_CREDS_KEY, b64);
}

function clearCreds() {
  sessionStorage.removeItem(VAULT_CREDS_KEY);
  sessionStorage.removeItem(VAULT_JUST_REGISTERED_KEY);
}

function hasAuth() {
  return !!getCreds();
}

function authHeaders() {
  const creds = getCreds();
  return creds ? { Authorization: 'Basic ' + creds } : {};
}

// Any API 401 means the stored credentials stopped working — drop them and
// return to the login screen. The unlock view is exempt: it never calls the
// daemon with box creds, and its account session is managed separately.
const ACCT_SERVER = 'https://sync.ellmstack.dev';  // account API origin (same relay the login/unlock routes use)

// Global account menu (shell topbar, every route): signed-in email, a link
// to the Account & Sync settings, and a reliable Sign out. Always mounted —
// signed-out shows "Sign in" — visible on the console too, not only on the
// login/unlock screens.
let globalChipMounted = false;
function mountAccountChip(anchor, server) {
  const btn = document.createElement('button');
  btn.className = 'acct-chip';
  const menu = document.createElement('div');
  menu.className = 'acct-menu';
  menu.style.display = 'none';
  anchor.appendChild(btn);
  anchor.appendChild(menu);
  const renderSignedOut = () => {
    btn.textContent = '\u{1F464} Sign in';
    btn.title = 'Sign in to your Engram account';
    menu.innerHTML = `<button class="acct-menu-item" id="acct-chip-signin">Sign in</button>`;
    menu.querySelector('#acct-chip-signin').onclick = () => {
      menu.style.display = 'none';
      navigate('#/login');
    };
  };
  const renderSignedIn = (email) => {
    btn.textContent = '\u{1F464} ' + (email || 'Account');
    btn.title = 'Account';
    menu.innerHTML = `
      <div class="acct-menu-head">Signed in as<br><span class="mono">${esc(email || 'account')}</span></div>
      <button class="acct-menu-item" id="acct-chip-settings">⚙ Account settings</button>
      <button class="acct-menu-item acct-menu-danger" id="acct-chip-signout">Sign out</button>`;
    menu.querySelector('#acct-chip-settings').onclick = () => {
      menu.style.display = 'none';
      navigate('#/settings');
    };
    menu.querySelector('#acct-chip-signout').onclick = async () => {
      try { await api.account.logout(server); } catch {}
      api.account.setToken(null);
      unlock.signOut();
      toast('Signed out', 'ok');
      renderSignedOut();
      navigate('#/login');
    };
  };
  renderSignedOut();
  // Truth-check any stored token on mount: a session revoked elsewhere (or
  // expired server-side) must read as signed out immediately, not after the
  // first click.
  if (api.account.token()) {
    api.account.credentials(server).then(c => {
      renderSignedIn(c && c.email ? c.email : null);
    }).catch((e) => {
      if (e && e.status === 401) { api.account.setToken(null); renderSignedOut(); }
    });
  }
  btn.onclick = (e) => {
    e.stopPropagation();
    const open = menu.style.display !== 'block';
    menu.style.display = open ? 'block' : 'none';
    if (open) {
      // Signed-in state is resolved on open — no flicker if the session
      // was created or destroyed in another tab since the last render.
      if (!api.account.token()) { renderSignedOut(); return; }
      api.account.credentials(server).then(c => {
        renderSignedIn(c && c.email ? c.email : null);
      }).catch((e) => {
        // A dead token (revoked remotely / expired) must read as signed
        // OUT — never show a phantom signed-in menu.
        if (e && e.status === 401) { api.account.setToken(null); renderSignedOut(); }
        else renderSignedIn(null);
      });
    }
  };
  document.addEventListener('click', (e) => {
    if (!anchor.contains(e.target)) menu.style.display = 'none';
  });
}

function syncGlobalChip() {
  const anchor = document.getElementById('global-acct-anchor');
  if (!anchor) return;
  // Always mounted: signed-out shows "Sign in", signed-in shows the
  // account menu. Never an empty gap in the topbar.
  if (!globalChipMounted) { mountAccountChip(anchor, ACCT_SERVER); globalChipMounted = true; }
}

function onApi401(r) {
  clearCreds();
  // A 401 can also mean the ACCOUNT session died server-side (e.g. revoked
  // remotely). Drop the stored token too — otherwise the login screen keeps
  // auto-forwarding on the dead token and bounces the user in a loop.
  api.account.setToken(null);
  const hash = (window.location.hash || '#/').replace(/^#/, '');
  if (hash !== '/login' && hash !== '/unlock' && !hash.startsWith('/reset/')) navigate('#/login');
}

// ── Router ────────────────────────────────────────────────────────────────

const routes = {};
function route(pattern, handler) {
  routes[pattern] = handler;
}

function navigate(hash) {
  window.location.hash = hash;
}

let currentCleanup = null;

async function render() {
  const hash = (window.location.hash || '#/').replace(/^#/, '');
  const app = document.getElementById('app');
  const statusbar = document.getElementById('statusbar');

  if (currentCleanup) { currentCleanup(); currentCleanup = null; }

  // Vault gate: everything except the login screen requires credentials.
  // The unlock view is also gate-exempt — it reads a synced vault straight
  // from the relay, decrypted in the browser, so box creds are irrelevant.
  // /reset/{token} is also gate-exempt: the reset email links to it and the
  // recipient may not be signed in anywhere.
  // The full tab bar is the product — box console users get it via box
  // creds, account users on the public site get it against their own
  // loopback daemon (see daemonApiBase). Only signed-out strangers get
  // no nav: the gate below bounces them to login.
  const mainNav = document.getElementById('main-nav');
  if (mainNav) mainNav.style.display = (hasAuth() || api.account.token()) ? '' : 'none';
  if (!hasAuth() && !api.account.token() && hash !== '/login' && hash !== '/unlock' && hash !== '/settings' && !hash.startsWith('/reset/') && !hash.startsWith('/handoff/')) {
    navigate('#/login');
    return;
  }

  // Highlight nav
  document.querySelectorAll('.nav a').forEach(a => {
    a.classList.toggle('active', a.getAttribute('href') === hash);
  });

  // Match route
  for (const [pattern, handler] of Object.entries(routes)) {
    const re = new RegExp('^' + pattern.replace(/:\w+/g, '([^/]+)') + '$');
    const m = hash.match(re);
    if (m) {
      try {
        app.innerHTML = '<div class="loading">Loading…</div>';
        const result = handler(...m.slice(1));
        if (result && typeof result.then === 'function') {
          const cleanup = await result;
          if (cleanup && typeof cleanup === 'function') currentCleanup = cleanup;
        }
      } catch (e) {
        // If a 401 cleared the creds mid-route, render() has already been
        // re-invoked for #/login — don't let the stale route clobber it.
        if (!hasAuth() && (window.location.hash || '#/').replace(/^#/, '') === '/login') return;
        app.innerHTML = `<div class="error-panel"><p>Error: ${esc(e.message)}</p></div>`;
      }
      syncGlobalChip();
      return;
    }
  }
  syncGlobalChip();
  app.innerHTML = '<div class="error-panel"><h2>404</h2><p>Page not found</p></div>';
}

window.addEventListener('hashchange', render);
window.addEventListener('DOMContentLoaded', render);

// ── Helpers ───────────────────────────────────────────────────────────────

function esc(s) {
  return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function ago(ts) {
  if (!ts) return '';
  const d = (Date.now() - new Date(ts).getTime()) / 1000;
  if (d < 60) return 'just now';
  if (d < 3600) return Math.floor(d / 60) + 'm ago';
  if (d < 86400) return Math.floor(d / 3600) + 'h ago';
  return Math.floor(d / 86400) + 'd ago';
}

function layerIcon(layer) {
  const icons = { episodic: '●', semantic: '◆', imagined: '✦' };
  return `<span class="layer-icon ${layer}">${icons[layer] || '●'}</span>`;
}

function layerBadge(layer) {
  return `<span class="badge badge-${layer}">${layerIcon(layer)} ${layer}</span>`;
}

function valenceLabel(v) {
  if (v >= 0.5) return '<span class="valence joyful">😊 Joyful</span>';
  if (v >= 0.1) return '<span class="valence positive">🙂 Positive</span>';
  if (v >= -0.3) return '<span class="valence neutral">😐 Neutral</span>';
  return '<span class="valence challenging">😟 Challenging</span>';
}

function strengthBar(s) {
  const pct = Math.min(100, Math.max(0, (s / 2) * 100));
  const color = pct > 60 ? 'var(--grounded)' : pct > 25 ? 'var(--episodic)' : 'var(--decaying)';
  return `<div class="mini-bar"><div class="mini-bar-fill" style="width:${pct}%;background:${color};"></div></div>`;
}

function tagList(tags) {
  if (!tags || !tags.length) return '';
  return tags.map(t => `<span class="tag">${esc(t)}</span>`).join('');
}

function sourceIcon(src) {
  const icons = { interaction: '💬', chat: '💭', window: '🖥', agent: '🤖', system: '⚙', consolidation: '🌙', imagined: '✦', research: '🔬', mic: '🎤', sensor: '📡' };
  return icons[src] || '●';
}

function resultLabel(t) {
  const labels = { fts5: 'FTS5', qem_cache: 'QEM', vector: 'VECTOR', like: 'LIKE', hybrid: 'HYBRID' };
  return labels[t] || t;
}

function toast(msg, kind = 'info') {
  const root = document.getElementById('toast-root');
  const el = document.createElement('div');
  el.className = `toast toast-${kind}`;
  el.textContent = msg;
  root.appendChild(el);
  setTimeout(() => { el.remove(); }, 3000);
}

/**
 * Shared capture path: POSTs a memory and surfaces the daemon's skip verdicts
 * (duplicate/noise) as a warn toast instead of a false "Captured". Returns the
 * API response, or null when the capture was skipped.
 */
async function captureMemory({ content, layer = 'episodic', tags = [] }) {
  const r = await api.memories.create({ content, layer, source: 'interaction', tags });
  if (r && r.skipped) {
    toast(r.skip_reason || 'Duplicate skipped', 'warn');
    return null;
  }
  return r;
}

// ── Live event stream (WebSocket + polling fallback) ─────────────────────

/**
 * Connect to /ws/events. Falls back to polling /memories/search every 5s
 * when the socket is down, and keeps trying to reconnect in the background.
 * onEvent({ type, memory, timestamp }) · onStatus('live'|'polling'|'reconnecting')
 * Returns { close() } — call from route cleanup.
 */
function connectEventStream({ onEvent, onStatus }) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const url = `${proto}//${location.host}/ws/events`;
  let ws = null;
  let stopped = false;
  let pollTimer = null;
  let retryTimer = null;

  const status = (s) => { if (onStatus) onStatus(s); };

  async function pollOnce() {
    try {
      const r = await api.memories.search({ sort_by: 'recency', limit: 10 });
      const list = Array.isArray(r) ? r : (r.results || []);
      // Oldest first so the feed prepends newest last; callers dedupe by id.
      for (let i = list.length - 1; i >= 0; i--) {
        onEvent({ type: 'capture', memory: list[i], timestamp: list[i].created_at });
      }
    } catch (e) { /* server down — keep polling */ }
  }

  function startPolling() {
    if (pollTimer || stopped) return;
    status('polling');
    pollTimer = setInterval(pollOnce, 5000);
    pollOnce();
  }

  function stopPolling() {
    if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
  }

  function scheduleRetry() {
    if (retryTimer || stopped) return;
    retryTimer = setTimeout(() => { retryTimer = null; connect(); }, 8000);
  }

  function connect() {
    if (stopped) return;
    status('reconnecting');
    try { ws = new WebSocket(url); } catch (e) { startPolling(); scheduleRetry(); return; }
    ws.onopen = () => { stopPolling(); status('live'); };
    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data);
        if (msg && msg.memory) onEvent(msg);
      } catch (e) { /* malformed frame */ }
    };
    ws.onclose = () => {
      if (stopped) return;
      startPolling();
      scheduleRetry();
    };
    ws.onerror = () => { try { ws.close(); } catch (e) { /* ignore */ } };
  }

  connect();

  return {
    close() {
      stopped = true;
      stopPolling();
      if (retryTimer) clearTimeout(retryTimer);
      if (ws) { ws.onclose = null; try { ws.close(); } catch (e) { /* ignore */ } }
    },
  };
}

// ── Sparkline (tiny SVG trend line, no axes) ─────────────────────────────

function sparkline(values, w = 100, h = 30) {
  if (!values || !values.length) return '';
  const max = Math.max(...values, 1e-9);
  const min = Math.min(...values, 0);
  const span = (max - min) || 1;
  const step = w / Math.max(1, values.length - 1);
  const pts = values.map((v, i) =>
    `${(i * step).toFixed(1)},${(h - 2 - ((v - min) / span) * (h - 4)).toFixed(1)}`);
  return `<svg class="sparkline" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" aria-hidden="true">` +
    `<polyline points="${pts.join(' ')}" fill="none" stroke="var(--semantic)" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round"/></svg>`;
}

// ── Activity line chart (captures vs retrievals) ─────────────────────────

function activityChart(days) {
  if (!days || !days.length) {
    return '<div class="faint" style="padding:1rem;">No activity data yet.</div>';
  }
  const W = 640, H = 220, padL = 36, padR = 10, padT = 12, padB = 26;
  const iw = W - padL - padR, ih = H - padT - padB;
  const maxV = Math.max(1, ...days.map(d => Math.max(d.captures || 0, d.retrievals || 0)));
  const x = (i) => padL + (i / Math.max(1, days.length - 1)) * iw;
  const y = (v) => padT + ih - (v / maxV) * ih;
  const line = (key) => days.map((d, i) =>
    `${i === 0 ? 'M' : 'L'}${x(i).toFixed(1)},${y(d[key] || 0).toFixed(1)}`).join(' ');

  let grid = '';
  for (let g = 0; g <= 4; g++) {
    const v = (maxV / 4) * g;
    const gy = y(v).toFixed(1);
    grid += `<line class="chart-grid" x1="${padL}" y1="${gy}" x2="${W - padR}" y2="${gy}"/>` +
      `<text class="chart-text" x="${padL - 6}" y="${+gy + 3}" text-anchor="end">${Math.round(v)}</text>`;
  }
  let xlabels = '';
  const step = Math.ceil(days.length / 6);
  days.forEach((d, i) => {
    if (i % step === 0) {
      xlabels += `<text class="chart-text" x="${x(i).toFixed(1)}" y="${H - 8}" text-anchor="middle">${esc((d.date || '').slice(5))}</text>`;
    }
  });

  return `<svg class="activity-chart" viewBox="0 0 ${W} ${H}" role="img" aria-label="Memory activity, last ${days.length} days">
    ${grid}
    <line class="chart-axis" x1="${padL}" y1="${padT}" x2="${padL}" y2="${padT + ih}"/>
    <line class="chart-axis" x1="${padL}" y1="${padT + ih}" x2="${W - padR}" y2="${padT + ih}"/>
    ${xlabels}
    <path d="${line('captures')}" fill="none" stroke="var(--episodic)" stroke-width="2" stroke-linejoin="round"/>
    <path d="${line('retrievals')}" fill="none" stroke="var(--semantic)" stroke-width="2" stroke-linejoin="round"/>
  </svg>`;
}

// ── Layer distribution (horizontal stacked bar) ───────────────────────────

function layerDistribution(byLayer) {
  const e = byLayer?.episodic || 0, s = byLayer?.semantic || 0, i = byLayer?.imagined || 0;
  const total = (e + s + i) || 1;
  const pct = (n) => (n / total) * 100;
  return `
    <div class="stacked-bar" role="img" aria-label="Layer distribution">
      <div class="seg episodic" style="width:${pct(e)}%"></div>
      <div class="seg semantic" style="width:${pct(s)}%"></div>
      <div class="seg imagined" style="width:${pct(i)}%"></div>
    </div>
    <div class="stacked-legend">
      <span>${layerIcon('episodic')} Episodic <b>${e}</b> · ${pct(e).toFixed(1)}%</span>
      <span>${layerIcon('semantic')} Semantic <b>${s}</b> · ${pct(s).toFixed(1)}%</span>
      <span>${layerIcon('imagined')} Imagined <b>${i}</b> · ${pct(i).toFixed(1)}%</span>
    </div>`;
}

// ── Shared feed/cards bits ────────────────────────────────────────────────

function notesBadge(m) {
  const n = m.annotations_count ?? m.annotation_count ??
    (Array.isArray(m.annotations) ? m.annotations.length : 0);
  return n > 0 ? `<span class="badge badge-notes" title="${n} note${n > 1 ? 's' : ''}">📝 ${n}</span>` : '';
}

function liveFeedItem(m) {
  const content = m.content || '';
  return `
    <a href="#/memories/${m.id}" class="feed-item live-item">
      ${layerIcon(m.layer)}
      <span class="faint" title="${esc(m.source || '?')}">${sourceIcon(m.source)}</span>
      <span class="feed-text">${esc(content.slice(0, 120))}${content.length > 120 ? '…' : ''}</span>
      <span class="faint ml-auto">${ago(m.created_at)}</span>
    </a>`;
}

// ── Status bar ────────────────────────────────────────────────────────────

async function updateStatus() {
  if (!hasAuth()) return;  // not signed in — the login screen covers the app
  try {
    const h = await api.health();
    const el = document.getElementById('statusbar');
    el.innerHTML = `
      <span class="status-item ok">● Connected</span>
      <span class="status-item">${h.memories_total || 0} memories</span>
      <span class="status-item">${qemStatus(h)}</span>
      <span class="status-item">${formatBytes(h.db_size_bytes || 0)}</span>
      <span class="status-item ok">Encrypted ✓</span>`;
  } catch (e) {
    const el = document.getElementById('statusbar');
    el.innerHTML = '<span class="status-item warn">⚠ Server unreachable</span>';
  }
}

function formatBytes(b) {
  if (b < 1024) return b + ' B';
  if (b < 1024 * 1024) return (b / 1024).toFixed(1) + ' KB';
  return (b / (1024 * 1024)).toFixed(1) + ' MB';
}

// QEM L1 status label: real hit rate only once lookups have happened;
// otherwise "warm" (cache populated, no traffic yet) or "cold".
function qemStatus(h) {
  const hits = h?.qem_hits || 0;
  const misses = h?.qem_misses || 0;
  const total = hits + misses;
  if (total > 0) return `QEM ${Math.round((hits / total) * 100)}%`;
  if ((h?.qem_cache_entries || 0) > 0) return 'QEM warm';
  return 'QEM cold';
}

// ── Modal ─────────────────────────────────────────────────────────────────

function showModal(title, bodyHtml) {
  const root = document.getElementById('modal-root');
  // CSP note: no inline event handlers — closing is handled by the delegated
  // click listener on #modal-root registered at init.
  root.innerHTML = `
    <div class="modal-overlay">
      <div class="modal">
        <div class="modal-header">
          <h2>${title}</h2>
          <button class="modal-close">&times;</button>
        </div>
        <div class="modal-body">${bodyHtml}</div>
      </div>
    </div>`;
}

/**
 * Global quick capture (topbar ＋ / Ctrl+Cmd+K): one textarea, a layer
 * selector and a tags field. On skip (duplicate/noise) the modal stays open
 * so the content isn't lost; on success it closes.
 */
function openCaptureModal() {
  showModal('Quick Capture', `
    <textarea id="cap-input" rows="3" placeholder="What's happening?"></textarea>
    <div class="cap-row">
      <select id="cap-layer" class="filter-select">
        <option value="episodic">Episodic — moment</option>
        <option value="semantic">Semantic — durable note</option>
        <option value="imagined">Imagined — idea to explore</option>
      </select>
      <input id="cap-tags" class="input-sm" placeholder="tags, comma, separated">
    </div>
    <div class="mutation"><button class="btn btn-primary" id="cap-submit">Capture</button></div>`);
  const submit = async () => {
    const input = document.getElementById('cap-input');
    const content = input.value.trim();
    if (!content) return;
    const layer = document.getElementById('cap-layer').value;
    const tags = document.getElementById('cap-tags').value.split(',').map(s => s.trim()).filter(Boolean);
    try {
      const r = await captureMemory({ content, layer, tags });
      if (r === null) return; // skipped — toast shown, keep the text
      document.getElementById('modal-root').innerHTML = '';
      toast('Captured', 'ok');
    } catch (e) {
      toast('Capture failed: ' + e.message, 'error');
    }
  };
  document.getElementById('cap-submit').onclick = submit;
  const ta = document.getElementById('cap-input');
  ta.focus();
  ta.onkeydown = (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) submit();
  };
}

// ==========================================================================
// SCREENS
// ==========================================================================

// ── Login ─────────────────────────────────────────────────────────────────
// Branded vault gate replacing the native basic-auth popup. The form sends
// an explicit Authorization header — browsers never show the popup when a
// request already carries credentials, and Caddy strips the challenge from
// API 401s anyway. No headerless auto-enter probe: browsers silently attach
// cached basic-auth, so a probe would bounce every login-screen visit into
// the console while box creds are cached (account form unreachable).
//
// The login screen is also the site's front door for Engram ACCOUNTS
// (roadmap 1.2): "Create an account" registers a passkey against the
// public relay, then lands on the Add-device wizard (WARP-style pairing
// code). No vault credentials are needed for any of that — the relay calls
// go straight from the browser, and the hosted site's relay is fixed.

route('/login', () => {
  const app = document.getElementById('app');
  // The relay behind the hosted site is fixed; the Settings panel uses the
  // daemon's configured server_url for custom-relay users.
  const PAIR_CODE_SERVER = 'https://sync.ellmstack.dev';

  // ── Account key A (email+password accounts) ──────────────────────────────
  // A (32 random bytes) is generated client-side, shown ONCE as a 12-word
  // recovery phrase, and wrapped twice: under the password and under the
  // phrase. The relay stores only the AES-GCM ciphertexts — it can never
  // open them (zero-knowledge preserved). The phrase is never sent anywhere
  // in full; only its ciphertext and a random salt reach the relay.

  function generateRecoveryPhrase() {
    // 12 random words from the official BIP39 list (2048 words). 2048
    // divides 2^32 exactly, so `rnd[i] % 2048` has no modulo bias.
    const rnd = new Uint32Array(12);
    crypto.getRandomValues(rnd);
    const words = [];
    for (let i = 0; i < 12; i++) words.push(BIP39_WORDS[rnd[i] % 2048]);
    return words;
  }

  async function storeAccountKeyWraps(server, accountId, A, password, phraseWords) {
    const phrase = phraseWords.join(' ');
    const saltPw = crypto.getRandomValues(new Uint8Array(16));
    const saltRec = crypto.getRandomValues(new Uint8Array(16));
    const wkPw = await unlock.deriveWithSalt(password, saltPw);
    const wkRec = await unlock.deriveWithSalt(phrase, saltRec);
    try {
      const wrappedA = await unlock.wrapKey(wkPw, A);
      const wrappedARec = await unlock.wrapKey(wkRec, A);
      // Recovery wrap first: the phrase is the most precious credential.
      await api.account.putRecoveryWrap(server, unlock.b64encode(wrappedARec), unlock.b64encode(saltRec));
      await api.account.putPasswordWrap(server, unlock.b64encode(wrappedA), unlock.b64encode(saltPw));
    } finally {
      wkPw.fill(0); wkRec.fill(0);
    }
    unlock.setAccountKey(accountId, A);
  }

  // "Save this phrase" interstitial — shown right after signup, and after
  // any signin that finds the account has no key wraps yet (aborted signup).
  // onDone runs AFTER the wraps are stored server-side.
  function renderPhraseGate(server, accountId, email, password, onDone) {
    const words = generateRecoveryPhrase();
    app.innerHTML = `
      <div class="modal-overlay">
        <div class="modal login-modal">
          <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
          <div class="modal-body">
            <p style="margin-top:0;">This is your <strong>account recovery phrase</strong> — the only way back into your vaults if you forget your password. <strong>Write it down now.</strong> It is shown exactly once and is never stored anywhere.</p>
            <div class="recovery-phrase">${words.map(w => `<span class="recovery-word">${w}</span>`).join('')}</div>
            <div class="mutation">
              <button class="btn btn-sm" id="phrase-copy">Copy phrase</button>
            </div>
            <p class="faint">To prove you saved it, type the first and last words:</p>
            <input id="phrase-w1" type="text" placeholder="Word 1" autocomplete="off" autocapitalize="off" spellcheck="false">
            <input id="phrase-w12" type="text" placeholder="Word 12" autocomplete="off" autocapitalize="off" spellcheck="false">
            <div id="phrase-error"></div>
            <div class="mutation">
              <button class="btn btn-primary" id="phrase-go">I saved it — continue</button>
            </div>
          </div>
        </div>
      </div>`;
    document.getElementById('phrase-copy').onclick = async () => {
      try { await navigator.clipboard.writeText(words.join(' ')); toast('Phrase copied', 'ok'); }
      catch { toast('Copy blocked by the browser — type it manually', 'error'); }
    };
    document.getElementById('phrase-go').onclick = async () => {
      const a = document.getElementById('phrase-w1').value.trim().toLowerCase();
      const b = document.getElementById('phrase-w12').value.trim().toLowerCase();
      if (a !== words[0] || b !== words[11]) {
        document.getElementById('phrase-error').innerHTML =
          `<div class="error-panel"><p>Those words don't match. Check the phrase above.</p></div>`;
        return;
      }
      const btn = document.getElementById('phrase-go');
      btn.disabled = true;
      try {
        const A = crypto.getRandomValues(new Uint8Array(32));
        await storeAccountKeyWraps(server, accountId, A, password, words);
        A.fill(0);
        onDone();
      } catch (e) {
        btn.disabled = false;
        document.getElementById('phrase-error').innerHTML = `<div class="error-panel"><p>${esc(e.message)}</p></div>`;
      }
    };
  }

  // After signin: put A in memory if the password opens the wrap. Returns
  // 'ok' (A in memory), 'setup' (no wraps yet — aborted signup), or
  // 'recovery' (the wrap exists but this password can't open it — the
  // classic post-reset state).
  async function acquireAccountKey(server, accountId, password) {
    const wraps = await api.account.wraps(server);
    if (!wraps.password_wrap) return 'setup';
    try {
      const blob = await api.account.getPasswordWrap(server);
      const wk = await unlock.deriveWithSalt(password, unlock.b64decode(blob.salt_pw));
      const A = await unlock.unwrapKey(wk, unlock.b64decode(blob.wrapped_a));
      wk.fill(0);
      unlock.setAccountKey(accountId, A);
      A.fill(0);
      return 'ok';
    } catch {
      return 'recovery';
    }
  }

  // Post-reset: the new password cannot open the old password wrap. The
  // recovery phrase unwraps A, then A is re-wrapped under the new password.
  function renderRecoveryGate(server, accountId, newPassword) {
    app.innerHTML = `
      <div class="modal-overlay">
        <div class="modal login-modal">
          <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
          <div class="modal-body">
            <p style="margin-top:0;">Your password changed, but your vault keys are still wrapped under the old one. Enter your <strong>12-word recovery phrase</strong> to re-link them. The phrase is used only in this tab — nothing is sent to the server.</p>
            <input id="rec-phrase" type="text" placeholder="12-word recovery phrase" autocomplete="off" autocapitalize="off" spellcheck="false">
            <div id="rec-error"></div>
            <div class="mutation">
              <button class="btn btn-primary" id="rec-go">Re-link my vault keys</button>
              <button class="btn" id="rec-skip">Skip — my vaults stay locked</button>
            </div>
          </div>
        </div>
      </div>`;
    const fail = (msg) => {
      document.getElementById('rec-error').innerHTML = `<div class="error-panel"><p>${esc(msg)}</p></div>`;
    };
    document.getElementById('rec-go').onclick = async () => {
      const phrase = document.getElementById('rec-phrase').value.trim().toLowerCase().replace(/\s+/g, ' ');
      if (phrase.split(' ').length !== 12) { fail('That is not a 12-word phrase.'); return; }
      const btn = document.getElementById('rec-go');
      btn.disabled = true;
      let A;
      try {
        const blob = await api.account.getRecoveryWrap(server);
        const wk = await unlock.deriveWithSalt(phrase, unlock.b64decode(blob.salt_rec));
        A = await unlock.unwrapKey(wk, unlock.b64decode(blob.wrapped_a_rec));
        wk.fill(0);
      } catch (e) {
        if (e.status === 404) { fail('No recovery phrase is stored for this account — your vaults stay locked.'); return; }
        if (e.status === 401) { toast('Account session expired — sign in again', 'error'); signedOut(); return; }
        btn.disabled = false;
        fail('That phrase does not match this account.');
        return;
      }
      try {
        // Re-wrap A under the new password so signin opens vaults again.
        const saltPw = crypto.getRandomValues(new Uint8Array(16));
        const wkPw = await unlock.deriveWithSalt(newPassword, saltPw);
        try {
          const wrappedA = await unlock.wrapKey(wkPw, A);
          await api.account.putPasswordWrap(server, unlock.b64encode(wrappedA), unlock.b64encode(saltPw));
        } finally {
          wkPw.fill(0);
        }
        unlock.setAccountKey(accountId, A);
        A.fill(0);
        toast('Vault keys re-linked', 'ok');
        navigate('#/unlock');
      } catch (e) {
        if (e.status === 401) { toast('Account session expired — sign in again', 'error'); signedOut(); return; }
        btn.disabled = false;
        fail(e.message);
      }
    };
    document.getElementById('rec-skip').onclick = () => navigate('#/unlock');
  }

  const renderLoginView = (view) => {
    // Any deliberate exit from the wizard (back, sign-out, expired session,
    // passkey login) dismisses it — it must not resurface on the next load.
    if (view !== 'pair') sessionStorage.removeItem(VAULT_JUST_REGISTERED_KEY);
    if (view === 'pair') {
      app.innerHTML = `
        <div class="modal-overlay">
          <div class="modal login-modal">
            <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
            <div class="modal-body">
              <p class="faint" style="margin-top:0;">Signed in<span id="pair-acct"></span>. Now link this machine:</p>
              <div class="mutation">
                <button class="btn btn-primary" id="pair-mint">Pair a device</button>
              </div>
              <div id="pair-once"></div>
              <div class="login-alt">
                <p class="faint"><a href="#" id="pair-back">← Back to vault sign in</a> · <a href="#" id="pair-logout">Sign out</a></p>
              </div>
            </div>
          </div>
        </div>`;
      document.getElementById('pair-mint').onclick = async () => {
        const el = document.getElementById('pair-once');
        el.innerHTML = '<div class="faint">Requesting a pairing code…</div>';
        try {
          const res = await api.account.pairCodes(PAIR_CODE_SERVER);
          el.innerHTML = pairCodeHtml(res.code);
          wirePairCodeCopies(el);
          sessionStorage.setItem(PAIR_POLL_KEY, '1');
        } catch (e) {
          if (e.status === 401) {
            api.account.setToken(null);
            toast('Account session expired — sign in again', 'error');
            renderLoginView('account');
            return;
          }
          el.innerHTML = `<div class="error-panel"><p>${esc(e.message)}</p></div>`;
        }
      };
      document.getElementById('pair-back').onclick = (e) => { e.preventDefault(); renderLoginView('account'); };
      document.getElementById('pair-logout').onclick = async (e) => {
        e.preventDefault();
        try { await api.account.logout(PAIR_CODE_SERVER); } catch {}
        api.account.setToken(null);
        toast('Signed out', 'ok');
        renderLoginView('account');
      };
      // Validate the session and show the account id (401 → back to form).
      (async () => {
        try {
          const acct = await api.account.get(PAIR_CODE_SERVER);
          const el = document.getElementById('pair-acct');
          if (el) el.textContent = ' as ' + (acct.account_id || '').slice(0, 13) + '…';
        } catch (e) {
          if (e.status === 401) {
            api.account.setToken(null);
            toast('Account session expired — sign in again', 'error');
            renderLoginView('account');
          }
        }
      })();
    } else if (view === 'signin') {
      // Email + password is the primary credential; the passkey is a faster
      // secondary way in. On success the account key A is unwrapped from the
      // password wrap so open vaults need no further prompts.
      app.innerHTML = `
        <div class="modal-overlay">
          <div class="modal login-modal">
            <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
            <div class="modal-body">
              <p class="faint" style="margin-top:0;">Sign in to your Engram account.</p>
              <input id="login-email" type="email" placeholder="Email" autocomplete="username" autocapitalize="off" spellcheck="false">
              <input id="login-password" type="password" placeholder="Password" autocomplete="current-password">
              <div id="login-error"></div>
              <div class="mutation">
                <button class="btn btn-primary" id="login-submit">Sign in</button>
                <button class="btn" id="login-passkey">Sign in with passkey</button>
              </div>
              <div class="login-alt">
                <p class="faint"><a href="#" id="login-forgot">Forgot your password?</a> · <a href="#" id="login-signup">Create an account</a></p>
                <p class="faint">Just viewing? <a href="#/unlock">Unlock a synced vault in this browser</a> — read-only, your vault passphrase decrypts it locally.</p>
                <p class="faint"><a href="#" id="login-operator">Server operator? Sign in to this server's own vault</a> — separate credentials.</p>
              </div>
            </div>
          </div>
        </div>`;
      const fail = (msg) => {
        document.getElementById('login-error').innerHTML = `<div class="error-panel"><p>${esc(msg)}</p></div>`;
      };
      const submit = async () => {
        const email = document.getElementById('login-email').value.trim().toLowerCase();
        const password = document.getElementById('login-password').value;
        if (!email || !password) return;
        const btn = document.getElementById('login-submit');
        btn.disabled = true;
        try {
          const res = await api.account.signin(PAIR_CODE_SERVER, email, password);
          api.account.setToken(res.session_token);
          const keyState = await acquireAccountKey(PAIR_CODE_SERVER, res.account_id, password);
          if (keyState === 'setup') {
            // Signup was aborted before the wraps landed — create the key now.
            renderPhraseGate(PAIR_CODE_SERVER, res.account_id, email, password, () => { if (resumePendingHandoff()) return; navigate('#/unlock'); });
          } else if (keyState === 'recovery') {
            renderRecoveryGate(PAIR_CODE_SERVER, res.account_id, password);
          } else {
            if (resumePendingHandoff()) return;
            navigate('#/unlock');
          }
        } catch (e) {
          if (e.status === 401) fail('Incorrect email or password.');
          else if (e.status === 429) fail('Too many attempts — wait a few minutes.');
          else fail(e.message);
          btn.disabled = false;
        }
      };
      document.getElementById('login-submit').onclick = submit;
      document.getElementById('login-password').onkeydown = (e) => { if (e.key === 'Enter') submit(); };
      document.getElementById('login-email').focus();
      document.getElementById('login-passkey').onclick = () => webauthnLogin(PAIR_CODE_SERVER);
      document.getElementById('login-forgot').onclick = (e) => { e.preventDefault(); renderLoginView('forgot'); };
      document.getElementById('login-signup').onclick = (e) => { e.preventDefault(); renderLoginView('signup'); };
      document.getElementById('login-operator').onclick = (e) => { e.preventDefault(); renderLoginView('operator'); };
    } else if (view === 'signup') {
      app.innerHTML = `
        <div class="modal-overlay">
          <div class="modal login-modal">
            <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
            <div class="modal-body">
              <p class="faint" style="margin-top:0;">Create your Engram account — an email and a password. Vaults you own open themselves; the password never leaves this browser unhashed, and the relay can't read your data.</p>
              <input id="signup-email" type="email" placeholder="Email" autocomplete="username" autocapitalize="off" spellcheck="false">
              <input id="signup-password" type="password" placeholder="Password (12+ characters)" autocomplete="new-password">
              <input id="signup-confirm" type="password" placeholder="Confirm password" autocomplete="new-password">
              <div id="signup-error"></div>
              <div class="mutation">
                <button class="btn btn-primary" id="signup-go">Create account</button>
              </div>
              <div class="login-alt">
                <p class="faint"><a href="#" id="signup-back">← Back to sign in</a></p>
              </div>
            </div>
          </div>
        </div>`;
      const fail = (msg) => {
        document.getElementById('signup-error').innerHTML = `<div class="error-panel"><p>${esc(msg)}</p></div>`;
      };
      const submit = async () => {
        const email = document.getElementById('signup-email').value.trim().toLowerCase();
        const password = document.getElementById('signup-password').value;
        const confirm = document.getElementById('signup-confirm').value;
        if (!email || !password) return;
        if (password.length < 12) { fail('Password must be at least 12 characters.'); return; }
        if (password !== confirm) { fail('Passwords do not match.'); return; }
        const btn = document.getElementById('signup-go');
        btn.disabled = true;
        try {
          const res = await api.account.signup(PAIR_CODE_SERVER, email, password);
          api.account.setToken(res.session_token);
          renderPhraseGate(PAIR_CODE_SERVER, res.account_id, email, password, () => { if (resumePendingHandoff()) return; navigate('#/unlock'); });
        } catch (e) {
          if (e.code === 'email_taken') fail('An account with that email already exists — sign in instead.');
          else if (e.code === 'weak_password') fail('Password must be 12–128 characters.');
          else if (e.code === 'invalid_email') fail('Enter a valid email address.');
          else if (e.status === 429) fail('Too many attempts — wait a few minutes.');
          else fail(e.message);
          btn.disabled = false;
        }
      };
      document.getElementById('signup-go').onclick = submit;
      document.getElementById('signup-confirm').onkeydown = (e) => { if (e.key === 'Enter') submit(); };
      document.getElementById('signup-email').focus();
      document.getElementById('signup-back').onclick = (e) => { e.preventDefault(); renderLoginView('signin'); };
    } else if (view === 'forgot') {
      // Two paths: email reset link (when the relay has SMTP configured —
      // `sent: true` is returned regardless of account existence, so we
      // cannot leak which emails exist), or an operator-issued token pasted
      // below. Honest copy: the relay cannot recover vault keys either way.
      app.innerHTML = `
        <div class="modal-overlay">
          <div class="modal login-modal">
            <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
            <div class="modal-body">
              <p class="faint" style="margin-top:0;">Reset your account password. Your vaults stay encrypted — after resetting, your recovery phrase re-links them.</p>
              <input id="forgot-email" type="email" placeholder="Email" autocomplete="username" autocapitalize="off" spellcheck="false">
              <div class="mutation">
                <button class="btn btn-primary" id="forgot-send">Send reset link</button>
              </div>
              <div id="forgot-result"></div>
              <hr>
              <p class="faint">Have a reset token? (Your server operator can issue one.)</p>
              <input id="forgot-token" type="text" placeholder="Reset token" autocomplete="off" autocapitalize="off" spellcheck="false">
              <input id="forgot-newpass" type="password" placeholder="New password (12+ characters)" autocomplete="new-password">
              <div id="forgot-error"></div>
              <div class="mutation">
                <button class="btn" id="forgot-confirm">Reset password</button>
              </div>
              <div class="login-alt">
                <p class="faint"><a href="#" id="forgot-back">← Back to sign in</a></p>
              </div>
            </div>
          </div>
        </div>`;
      document.getElementById('forgot-send').onclick = async () => {
        const email = document.getElementById('forgot-email').value.trim().toLowerCase();
        if (!email) return;
        const btn = document.getElementById('forgot-send');
        const el = document.getElementById('forgot-result');
        btn.disabled = true;
        try {
          const res = await api.account.resetRequest(PAIR_CODE_SERVER, email);
          if (res.sent) {
            el.innerHTML = '<div class="settings-note"><div class="faint">✓ If that email has an account, a reset link is on its way. It expires in 30 minutes.</div></div>';
          } else {
            el.innerHTML = `<div class="settings-note"><div class="faint">This relay has no email configured. Ask your server operator to run <span class="mono">engramd-sync admin reset-token ${esc(email)}</span> — they will hand you a token to paste below.</div></div>`;
          }
        } catch (e) {
          el.innerHTML = `<div class="error-panel"><p>${esc(e.message)}</p></div>`;
        } finally {
          btn.disabled = false;
        }
      };
      document.getElementById('forgot-confirm').onclick = async () => {
        const token = document.getElementById('forgot-token').value.trim();
        const np = document.getElementById('forgot-newpass').value;
        const err = document.getElementById('forgot-error');
        if (!token || np.length < 12) { err.innerHTML = '<div class="error-panel"><p>Enter the reset token and a new password (12+ characters).</p></div>'; return; }
        const btn = document.getElementById('forgot-confirm');
        btn.disabled = true;
        try {
          await api.account.resetConfirm(PAIR_CODE_SERVER, token, np);
          toast('Password reset — sign in with the new one', 'ok');
          renderLoginView('signin');
        } catch (e) {
          if (e.status === 401) err.innerHTML = '<div class="error-panel"><p>That token is invalid or expired.</p></div>';
          else if (e.code === 'weak_password') err.innerHTML = '<div class="error-panel"><p>Password must be 12–128 characters.</p></div>';
          else err.innerHTML = `<div class="error-panel"><p>${esc(e.message)}</p></div>`;
          btn.disabled = false;
        }
      };
      document.getElementById('forgot-back').onclick = (e) => { e.preventDefault(); renderLoginView('signin'); };
    } else if (view === 'account') {
      // Front door: a verified session lands on an account banner;
      // otherwise email+password signin is the primary credential (the
      // passkey is a secondary method, attached from Settings → Account).
      if (api.account.token()) {
        app.innerHTML = `
          <div class="modal-overlay">
            <div class="modal login-modal">
              <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
              <div class="modal-body">
                <p class="faint" style="margin-top:0;">Your account syncs your vaults across devices.</p>
                <div id="login-acct"></div>
                <div class="login-alt">
                  <p class="faint">Just viewing? <a href="#/unlock">Unlock a synced vault in this browser</a> — read-only, your vault passphrase decrypts it locally.</p>
                  <p class="faint"><a href="#" id="login-operator">Server operator? Sign in to this server's own vault</a> — separate credentials.</p>
                </div>
              </div>
            </div>
          </div>`;
        document.getElementById('login-operator').onclick = (e) => { e.preventDefault(); renderLoginView('operator'); };
        const acctEl = document.getElementById('login-acct');
        acctEl.innerHTML = '<div class="faint">Checking account…</div>';
        (async () => {
          try {
            const acct = await api.account.get(PAIR_CODE_SERVER);
            acctEl.innerHTML = `
              <div class="settings-note">
                <div class="faint">✓ Signed in as <span class="mono">${esc((acct.account_id || '').slice(0, 13))}…</span></div>
                <div class="mutation" style="padding:0;">
                  <button class="btn btn-primary btn-sm" id="login-unlock">Unlock vault</button>
                  <button class="btn btn-sm" id="login-pair">Pair this machine</button>
                  <button class="btn btn-sm" id="login-acct-logout">Sign out</button>
                </div>
                <div id="login-pair-once"></div>
              </div>`;
            document.getElementById('login-unlock').onclick = () => navigate('#/unlock');
            document.getElementById('login-pair').onclick = async () => {
              const once = document.getElementById('login-pair-once');
              once.innerHTML = '<div class="faint">Requesting a pairing code…</div>';
              try {
                const res = await api.account.pairCodes(PAIR_CODE_SERVER);
                once.innerHTML = pairCodeHtml(res.code);
                wirePairCodeCopies(once);
                sessionStorage.setItem(PAIR_POLL_KEY, '1');
              } catch (e) {
                if (e.status === 401) {
                  api.account.setToken(null);
                  toast('Account session expired — sign in again', 'error');
                  renderLoginView('account');
                  return;
                }
                once.innerHTML = `<div class="error-panel"><p>${esc(e.message)}</p></div>`;
              }
            };
            document.getElementById('login-acct-logout').onclick = async (e) => {
              e.preventDefault();
              try { await api.account.logout(PAIR_CODE_SERVER); } catch {}
              api.account.setToken(null);
              unlock.signOut();
              toast('Signed out', 'ok');
              renderLoginView('account');
            };
          } catch (e) {
            if (e.status === 401) {
              api.account.setToken(null);
              renderLoginView('account');
            }
          }
        })();
        return;
      }
      renderLoginView('signin');
      return;
    } else {
      // Operator sign-in: this server's own vault (HTTP basic auth on the
      // box). Deliberately small and demoted — NOT the account flow.
      app.innerHTML = `
        <div class="modal-overlay">
          <div class="modal login-modal">
            <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
            <div class="modal-body">
              <p class="faint" style="margin-top:0;">This server's own vault (engramd) — separate credentials from your Engram account.</p>
              <input id="login-user" type="text" placeholder="Username" autocomplete="username" autocapitalize="off" spellcheck="false">
              <input id="login-pass" type="password" placeholder="Password" autocomplete="current-password">
              <div id="login-error"></div>
              <div class="mutation">
                <button class="btn btn-primary" id="login-submit">Sign in</button>
              </div>
              <div class="login-alt">
                <p class="faint"><a href="#" id="login-account-back">← Sign in with your account</a></p>
              </div>
            </div>
          </div>
        </div>`;

      const fail = (msg) => {
        document.getElementById('login-error').innerHTML = `<div class="error-panel"><p>${esc(msg)}</p></div>`;
      };

      const submit = async () => {
        const btn = document.getElementById('login-submit');
        const user = document.getElementById('login-user').value.trim();
        const pass = document.getElementById('login-pass').value;
        if (!user || !pass) return;
        const b64 = btoa(unescape(encodeURIComponent(user + ':' + pass)));
        btn.disabled = true;
        try {
          const r = await daemonFetch('/health', { headers: { Authorization: 'Basic ' + b64 } });
          if (r.ok) {
            setCreds(b64);
            sessionStorage.removeItem(VAULT_JUST_REGISTERED_KEY);
            updateStatus();
            navigate('#/');
            return;
          }
          if (r.status === 401) fail('Incorrect username or password.');
          else fail(`Sign-in failed (${r.status}).`);
        } catch (e) {
          fail('Cannot reach the vault server.');
        } finally {
          btn.disabled = false;
        }
      };

      document.getElementById('login-submit').onclick = submit;
      const onKey = (e) => { if (e.key === 'Enter') submit(); };
      document.getElementById('login-user').onkeydown = onKey;
      document.getElementById('login-pass').onkeydown = onKey;
      document.getElementById('login-user').focus();
      document.getElementById('login-account-back').onclick = (e) => { e.preventDefault(); renderLoginView('account'); };
    }
  };

  // The wizard shows only right after registration (flag set by
  // webauthnRegister). A lingering session token alone — e.g. a passkey
  // LOGIN, or any revisit within the 7-day session — lands on the form.
  //
  // No auto-enter probe here anymore: browsers silently attach cached
  // basic-auth to headerless fetches, so any /health probe would 200 while
  // box creds are cached and bounce the user straight into the console —
  // making the ACCOUNT form unreachable ("logs me right in on refresh").
  // Cached box creds still open the console directly on every other route
  // via hasAuth(); this screen stays the account front door.
  if (sessionStorage.getItem(VAULT_JUST_REGISTERED_KEY) === '1') renderLoginView('pair');
  else renderLoginView('account');

  // Pairing auto-advance: once a code is minted this visit, watch the
  // account's vaults — the moment the paired device registers on the
  // relay, move straight into the unlock picker. 401 = session died;
  // stop watching and fall back to the signin form.
  const poll = setInterval(async () => {
    if (sessionStorage.getItem(PAIR_POLL_KEY) !== '1') return;
    try {
      const vaults = await unlock.listVaults(PAIR_CODE_SERVER);
      if (vaults.length > 0) {
        sessionStorage.removeItem(PAIR_POLL_KEY);
        clearInterval(poll);
        toast('Device paired — vault synced', 'ok');
        navigate('#/unlock');
      }
    } catch (e) {
      if (e && e.status === 401) {
        sessionStorage.removeItem(PAIR_POLL_KEY);
        clearInterval(poll);
        api.account.setToken(null);
        renderLoginView('account');
      }
    }
  }, 3000);
  currentCleanup = () => clearInterval(poll);

  // A key-handoff link that bounced here signed-out resumes the moment a
  // session token exists (signin/signup paths also call this directly).
  if (resumePendingHandoff()) return;
});

// ── Password reset link (from the reset email: #/reset/{token}) ────────────
// Gate-exempt: the recipient may not be signed in anywhere. The token is
// single-use and 30-minute TTL (server-enforced); a reset revokes all other
// sessions, so the user lands back on signin afterwards.

// ── Vault key handoff from a local daemon (#/handoff/{token}?daemon=HOST:PORT) ──
// `engram handoff` mints a single-use token against this machine's daemon;
// this route redeems it and wraps the vault keys under the signed-in
// account key A — the vault becomes open-by-default with zero passphrase
// typing. The daemon runs on loopback; the token in the link is the secret.
route('/handoff/:token', (token) => {
  // The router's `:token` capture runs to the end of the hash, so a
  // `?daemon=...` suffix lands INSIDE the token string; the daemon then
  // sees a mangled token and reports it as expired/unknown. Strip the
  // query part — daemon defaults to 127.0.0.1:8799 anyway.
  token = token.split('?')[0];
  const app = document.getElementById('app');
  const ACCT = 'https://sync.ellmstack.dev';
  const daemon = (window.location.hash.match(/[?&]daemon=([^&]+)/) || [])[1] || '127.0.0.1:8799';
  const daemonOrigin = 'http://' + daemon;
  const fail = (msg) => {
    app.innerHTML = `
      <div class="modal-overlay">
        <div class="modal login-modal">
          <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
          <div class="modal-body">
            <div class="error-panel"><p>${esc(msg)}</p></div>
            <div class="login-alt">
              <p class="faint"><a href="#/login">← Back to sign in</a> · <a href="#/unlock">Go to vault picker</a></p>
            </div>
          </div>
        </div>
      </div>`;
  };
  app.innerHTML = `
    <div class="modal-overlay">
      <div class="modal login-modal">
        <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
        <div class="modal-body">
          <p class="faint" style="margin-top:0;">Linking this machine's vault keys to your account…</p>
          <div id="handoff-error"></div>
        </div>
      </div>
    </div>`;
  const el = document.getElementById('handoff-error');
  (async () => {
    try {
      const acct = await api.account.get(ACCT);
      const accountId = acct.account_id;
      // Chrome 142+ Local Network Access: a public site fetching a loopback
      // daemon needs targetAddressSpace:'loopback' AND the daemon's preflight
      // must carry Access-Control-Allow-Private-Network:true, or the fetch
      // dies with a bare network error that never reaches the daemon.
      let r;
      try {
        r = await fetch(
          new Request(daemonOrigin + '/sync/key-handoff/' + encodeURIComponent(token), {
            method: 'POST',
            targetAddressSpace: 'loopback',
          }));
      } catch {
        return fail('Your browser blocked the link to this machine\'s daemon (Chrome 142+ Local Network Access). Click the lock icon → Site settings → Local network access → Allow, then re-open the link. If the daemon isn\'t running, start it and run `engram handoff` again.');
      }
      if (!r.ok) {
        if (r.status === 401) return fail('That handoff link has expired or was already used — run `engram handoff` again.');
        return fail(`This machine's daemon refused the handoff (${r.status}).`);
      }
      // The redeem above validates the link BEFORE any password can be
      // asked: a dead link fails fast with nothing typed. Account key A
      // comes from memory first, else a signed-in tab of this origin shares
      // it over BroadcastChannel (still memory-only, never at rest), and
      // only then does the password prompt derive it.
      let A = unlock.getAccountKey(accountId);
      if (!A) A = await unlock.requestAccountKey(accountId);
      if (A) unlock.setAccountKey(accountId, A);
      if (!A) {
        const password = window.prompt('Enter your Engram account password to link the vault keys (it never leaves this tab):');
        if (!password) return fail('Account password required — run `engram handoff` again when ready.');
        const wraps = await api.account.wraps(ACCT);
        if (!wraps.password_wrap) return fail('Your account has no password wrap — sign in with your password first, then re-run `engram handoff`.');
        const blob = await api.account.getPasswordWrap(ACCT);
        const wk = await unlock.deriveWithSalt(password, unlock.b64decode(blob.salt_pw));
        try {
          A = await unlock.unwrapKey(wk, unlock.b64decode(blob.wrapped_a));
          unlock.setAccountKey(accountId, A);
        } catch {
          return fail('That password does not open your account key — run `engram handoff` again.');
        } finally { wk.fill(0); }
      }
      const h = await r.json();
      const enc = unlock.b64decode(h.enc_key_b64);
      const hmac = unlock.b64decode(h.hmac_key_b64);
      const vid = new TextEncoder().encode(String(h.vault_id || ''));
      const K = new Uint8Array(64 + vid.length);
      K.set(enc, 0); K.set(hmac, 32); K.set(vid, 64);
      const wrapped = await unlock.wrapKey(A, K);
      await api.account.putVaultWrap(ACCT, h.vault_id, unlock.b64encode(wrapped));
      enc.fill(0); hmac.fill(0); K.fill(0);
      // Strip the one-time token from history before moving on.
      history.replaceState(null, '', '#/handoff/');
      toast('Vault linked — it now opens with your account', 'ok');
      navigate('#/unlock');
    } catch (e) {
      if (e && e.status === 401) {
        // Signed-out (or stale session): keep the link alive across the
        // sign-in round-trip instead of destroying it.
        sessionStorage.setItem(HANDOFF_PENDING_KEY, JSON.stringify({ token, daemon }));
        api.account.setToken(null);
        navigate('#/login');
        return;
      }
      fail(e.message || 'Handoff failed.');
    }
  })();
});

route('/reset/:token', (token) => {
  const app = document.getElementById('app');
  // The reset email is sent by the hosted relay (same default the login
  // screen uses); custom-relay deployments mint their own links.
  const RESET_SERVER = 'https://sync.ellmstack.dev';
  app.innerHTML = `
    <div class="modal-overlay">
      <div class="modal login-modal">
        <div class="login-brand"><span class="brand-gem">◆</span> Engram Vault</div>
        <div class="modal-body">
          <p class="faint" style="margin-top:0;">Choose a new password. After the reset, your recovery phrase re-links your vault keys.</p>
          <input id="reset-newpass" type="password" placeholder="New password (12+ characters)" autocomplete="new-password">
          <input id="reset-confirm" type="password" placeholder="Confirm password" autocomplete="new-password">
          <div id="reset-error"></div>
          <div class="mutation">
            <button class="btn btn-primary" id="reset-go">Reset password</button>
          </div>
          <div class="login-alt">
            <p class="faint"><a href="#/login">← Back to sign in</a></p>
          </div>
        </div>
      </div>
    </div>`;
  const fail = (msg) => {
    document.getElementById('reset-error').innerHTML = `<div class="error-panel"><p>${esc(msg)}</p></div>`;
  };
  document.getElementById('reset-go').onclick = async () => {
    const np = document.getElementById('reset-newpass').value;
    const confirm = document.getElementById('reset-confirm').value;
    if (np.length < 12) { fail('Password must be 12–128 characters.'); return; }
    if (np !== confirm) { fail('Passwords do not match.'); return; }
    const btn = document.getElementById('reset-go');
    btn.disabled = true;
    try {
      await api.account.resetConfirm(RESET_SERVER, decodeURIComponent(token), np);
      api.account.setToken(null);
      toast('Password reset — sign in with the new one', 'ok');
      navigate('#/login');
    } catch (e) {
      if (e.status === 401) fail('That link is invalid or has expired — request a new one.');
      else fail(e.message);
      btn.disabled = false;
    }
  };
  document.getElementById('reset-confirm').onkeydown = (e) => { if (e.key === 'Enter') document.getElementById('reset-go').click(); };
  document.getElementById('reset-newpass').focus();
});

// ── Unlock (read-only synced vault, decrypted in the browser) ──────────────
// Gate-exempt view: pulls the account's encrypted blobs from the relay and
// decrypts client-side with the vault passphrase (js/unlock.js). Requires an
// account session (for pull); passphrase/keys never leave this tab and are
// wiped on lock/sign-out/reload.

route('/unlock', async () => {
  const app = document.getElementById('app');
  let relay = null;

  // Relay base: daemon config for custom-relay users. This view is
  // gate-exempt, so the config read can 401 (no box creds in this tab) —
  // fall back to the hosted relay in that case.
  async function relayUrl() {
    try {
      const cfg = await api.config.get();
      if (cfg && cfg.sync && cfg.sync.server_url) return cfg.sync.server_url;
    } catch {}
    return 'https://sync.ellmstack.dev';
  }

  function signedOut() {
    app.innerHTML = `
      <div class="page">
        <div class="panel" style="max-width:34rem;margin:2rem auto;">
          <div class="panel-header">Unlock a synced vault</div>
          <p class="faint" style="margin:0 1rem;">
            Read-only view of a vault synced to your Engram account — the vault
            passphrase decrypts everything in this browser. Nothing leaves this
            tab; the relay only ever sees ciphertext.
          </p>
          <div class="mutation" style="padding:1rem;">
            <button class="btn btn-primary" id="unlock-signin">Sign in with your account</button>
          </div>
        </div>
      </div>`;
    document.getElementById('unlock-signin').onclick = () => navigate('#/login');
  }

  // ── Open-by-default (account key A → vault keys) ────────────────────────
  // Open vaults decrypt without the vault passphrase: the relay holds the
  // vault key K = enc_key(32)‖hmac_key(32)‖vault_id(UTF-8) wrapped under
  // the account key A. A is in memory after password signin; passkey
  // signins prompt for the account password exactly once.

  let accountIdCache = null;
  let idleTimer = null;

  async function accountId() {
    if (!accountIdCache) {
      const acct = await api.account.get(relay);
      accountIdCache = acct.account_id;
    }
    return accountIdCache;
  }

  // Inactivity timer for the "lock after N minutes" policy: armed in list()
  // and re-armed by any activity while the timer policy is active.
  const onActivity = () => {
    if (!idleTimer || !unlock.isUnlocked()) return;
    const mins = parseInt(localStorage.getItem('engram-lock-policy:' + unlock.getMeta().vaultId) || '0', 10);
    if (!mins) return;
    clearTimeout(idleTimer);
    idleTimer = setTimeout(() => { unlock.lock(); toast('Locked after inactivity', 'info'); picker(); }, mins * 60000);
  };
  window.addEventListener('pointerdown', onActivity);
  window.addEventListener('keydown', onActivity);

  // Prompts for the ACCOUNT password (passkey signin: A isn't in this tab
  // yet). Resolves to A, or null if the user backs out.
  function accountPasswordOnce() {
    return new Promise((resolve) => {
      app.innerHTML = `
        <div class="page">
          <div class="panel" style="max-width:34rem;margin:2rem auto;">
            <div class="panel-header">Enter your account password</div>
            <div class="unlock-form">
              <p class="faint" style="margin:0 0 0.6rem;">You signed in with a passkey, so your account key isn't in this tab yet. Enter your <strong>account password</strong> (not a vault passphrase) once — it unlocks every open vault this session.</p>
              <input id="acct-pass" type="password" placeholder="Account password" autocomplete="current-password">
              <div id="acct-pass-error"></div>
              <div class="mutation">
                <button class="btn btn-primary" id="acct-pass-go">Unlock my account key</button>
                <button class="btn" id="acct-pass-back">Back</button>
              </div>
            </div>
          </div>
        </div>`;
      document.getElementById('acct-pass-go').onclick = async () => {
        const pass = document.getElementById('acct-pass').value;
        if (!pass) return;
        const btn = document.getElementById('acct-pass-go');
        btn.disabled = true;
        try {
          const id = await accountId();
          const blob = await api.account.getPasswordWrap(relay);
          const wk = await unlock.deriveWithSalt(pass, unlock.b64decode(blob.salt_pw));
          let A;
          try { A = await unlock.unwrapKey(wk, unlock.b64decode(blob.wrapped_a)); }
          finally { wk.fill(0); }
          unlock.setAccountKey(id, A);
          A.fill(0);
          resolve(unlock.getAccountKey(id));
        } catch (e) {
          if (e.status === 404) {
            document.getElementById('acct-pass-error').innerHTML =
              '<div class="error-panel"><p>This account has no password — locked vaults need the vault passphrase instead.</p></div>';
          } else if (e.status === 401) {
            toast('Account session expired — sign in again', 'error');
            signedOut();
            resolve(null);
            return;
          } else {
            document.getElementById('acct-pass-error').innerHTML =
              '<div class="error-panel"><p>Incorrect account password.</p></div>';
          }
          btn.disabled = false;
        }
      };
      document.getElementById('acct-pass').onkeydown = (e) => { if (e.key === 'Enter') document.getElementById('acct-pass-go').click(); };
      document.getElementById('acct-pass').focus();
      document.getElementById('acct-pass-back').onclick = () => { resolve(null); };
    });
  }

  async function openWithAccountKey(vaultId, label) {
    const id = await accountId();
    let A = unlock.getAccountKey(id);
    if (!A) {
      // No A: passkey signin, or this tab never saw the password. No
      // password wrap = passkey-only account → vault passphrase is the path.
      try { await api.account.getPasswordWrap(relay); }
      catch (e) {
        if (e.status === 404) {
          toast('This account has no password — unlock with the vault passphrase', 'error');
          passphrase(vaultId, label);
          return;
        }
        throw e;
      }
      A = await accountPasswordOnce();
      if (!A) return;  // backed out
    }
    let wrap;
    try { wrap = await api.account.getVaultWrap(relay, vaultId); }
    catch (e) {
      if (e.status === 404) { toast('This vault is locked — enter its passphrase', 'error'); passphrase(vaultId, label); return; }
      if (e.status === 401) { toast('Account session expired — sign in again', 'error'); signedOut(); return; }
      throw e;
    }
    const K = await unlock.unwrapKey(A, unlock.b64decode(wrap.wrapped_k));
    A.fill(0);
    // Composite K = enc(32)‖hmac(32)‖vault_id(UTF-8): verify the suffix so
    // a mis-stored wrap fails loudly instead of silently decrypting garbage.
    const idBytes = new TextEncoder().encode(vaultId);
    if (K.length < 64 + idBytes.length || !idBytes.every((b, i) => K[64 + i] === b)) {
      K.fill(0);
      toast('Stored vault key does not match this vault — unlocking with the passphrase instead', 'error');
      passphrase(vaultId, label);
      return;
    }
    app.innerHTML = `<div class="page"><div class="panel" style="max-width:34rem;margin:2rem auto;"><div class="panel-header">Opening ${esc(label)}…</div><div id="open-progress" class="faint">Downloading encrypted blobs…</div></div></div>`;
    try {
      await unlock.unlockWithKeys(relay, vaultId, { encRaw: K.slice(0, 32), hmacRaw: K.slice(32, 64) }, (p) => {
        const el = document.getElementById('open-progress');
        if (!el) return;
        if (p.stage === 'pulling') el.textContent = `Downloading encrypted blobs… ${p.fetched}`;
        else if (p.stage === 'verifying') el.textContent = `Verifying ${p.done}/${p.total}…`;
      });
    } catch (e) {
      if (e.status === 401) { toast('Account session expired — sign in again', 'error'); signedOut(); return; }
      toast(e.message, 'error');
      picker();
      return;
    }
    list(true);
  }

  // Wrap the OPEN vault's keys (from getVaultKeys) under A and store them:
  // this vault opens by default from now on. Called from the unlocked view
  // (only place K exists) and right after a passphrase unlock when the user
  // asked to open it by default.
  async function makeOpenByDefault(vaultId) {
    const keys = unlock.getVaultKeys();
    if (!keys) { toast('Vault is locked', 'error'); return; }
    const id = await accountId();
    let A = unlock.getAccountKey(id);
    if (!A) {
      try { await api.account.getPasswordWrap(relay); }
      catch (e) {
        if (e.status === 404) throw new Error('This account has no password — it cannot open vaults by default.');
        throw e;
      }
      A = await accountPasswordOnce();
      if (!A) return;
    }
    const idBytes = new TextEncoder().encode(vaultId);
    const K = new Uint8Array(64 + idBytes.length);
    K.set(keys.encRaw, 0); K.set(keys.hmacRaw, 32); K.set(idBytes, 64);
    keys.encRaw.fill(0); keys.hmacRaw.fill(0);
    const wrapped = await unlock.wrapKey(A, K);
    A.fill(0); K.fill(0);
    await api.account.putVaultWrap(relay, vaultId, unlock.b64encode(wrapped));
    toast('Vault now opens by default', 'ok');
  }

  async function toggleVaultWrap(vaultId, label, currentlyOpen) {
    if (!currentlyOpen) {
      // K only exists after a passphrase unlock — walk that flow first,
      // then wrap (passphrase() handles the wrap when asked to).
      passphrase(vaultId, label, { openByDefault: true });
      return;
    }
    let password = null;
    try {
      const creds = await api.account.credentials(relay);
      if (creds && creds.has_password) {
        password = prompt(`Locking "${label}" gates its keys behind the vault passphrase.\nConfirm with your account password:`);
        if (password === null) return;
        if (!password) { toast('Account password required to lock', 'error'); return; }
      }
    } catch {}
    if (!confirm(`Lock "${label}"?\nIt will need its vault passphrase every time. Memories are not re-encrypted.`)) return;
    try {
      await api.account.deleteVaultWrap(relay, vaultId, password);
      toast('Vault locked', 'ok');
      picker();
    } catch (e) {
      if (e.status === 401) { toast('Account session expired — sign in again', 'error'); signedOut(); return; }
      if (e.code === 'invalid_password') { toast('Incorrect account password', 'error'); return; }
      if (e.code === 'password_required') { toast('Enter your account password to lock', 'error'); return; }
      toast(e.message, 'error');
    }
  }

  function picker() {
    app.innerHTML = '<div class="loading">Loading vaults…</div>';
    unlock.listVaults(relay).then(vaults => {
      if (!vaults.length) {
        app.innerHTML = '<div class="error-panel"><p>No synced vaults for this account yet. Pair a device to start syncing.</p></div>';
        return;
      }
      app.innerHTML = `
        <div class="page">
          <div class="panel" style="max-width:40rem;margin:2rem auto;">
            <div class="panel-header">Unlock a synced vault</div>
            <div class="unlock-vaults">
              ${vaults.map(v => {
                const name = v.label || v.vault_id;
                const count = (typeof v.live_count === 'number') ? v.live_count : v.blob_count;
                const isOpen = !!v.is_open;
                return `
                <div class="unlock-vault-row">
                  <div>
                    <div class="unlock-vault-name">${esc(name)}${isOpen ? '' : ' <span class="faint" title="Locked — needs its vault passphrase">🔒</span>'}</div>
                    <div class="unlock-vault-id">${esc(v.vault_id)}</div>
                    <div class="faint">${count} ${count === 1 ? 'memory' : 'memories'} · synced ${ago(v.latest_sync)}</div>
                  </div>
                  <div class="unlock-vault-actions">
                    <button class="btn btn-primary btn-sm" data-unlock-vault="${esc(v.vault_id)}">${isOpen ? 'Open' : 'Unlock'}</button>
                    <button class="forget-link" data-toggle-vault="${esc(v.vault_id)}">${isOpen ? 'Lock' : 'Open by default'}</button>
                    <button class="forget-link" data-forget-vault="${esc(v.vault_id)}">Forget</button>
                  </div>
                </div>`;
              }).join('')}
            </div>
            <div class="unlock-footer">
              <span class="faint">Read-only — nothing leaves this browser.</span>
              <a href="#" id="unlock-signout">Sign out of account</a>
            </div>
          </div>
        </div>`;
      app.querySelectorAll('[data-unlock-vault]').forEach(btn => {
        const v = vaults.find(x => x.vault_id === btn.getAttribute('data-unlock-vault'));
        btn.onclick = () => v.is_open
          ? openWithAccountKey(v.vault_id, v.label || v.vault_id)
          : passphrase(v.vault_id, v.label || v.vault_id);
      });
      app.querySelectorAll('[data-toggle-vault]').forEach(btn => {
        const v = vaults.find(x => x.vault_id === btn.getAttribute('data-toggle-vault'));
        btn.onclick = () => toggleVaultWrap(v.vault_id, v.label || v.vault_id, !!v.is_open);
      });
      app.querySelectorAll('[data-forget-vault]').forEach(btn => {
        btn.onclick = async () => {
          const v = vaults.find(x => x.vault_id === btn.getAttribute('data-forget-vault'));
          if (!v) return;
          const count = (typeof v.live_count === 'number') ? v.live_count : v.blob_count;
          if (!confirm(`Forget "${v.label || v.vault_id}" (${v.vault_id})?\nThis permanently removes its ${count} synced ${count === 1 ? 'blob' : 'blobs'} from the relay.`)) return;
          btn.disabled = true;
          try {
            await unlock.forgetVault(relay, v.vault_id);
            toast('Vault forgotten', 'ok');
            picker();
          } catch (e) {
            if (e.status === 401) {
              toast('Account session expired — sign in again', 'error');
              signedOut();
              return;
            }
            toast(e.message, 'error');
            btn.disabled = false;
          }
        };
      });
      document.getElementById('unlock-signout').onclick = async (e) => {
        e.preventDefault();
        try { await api.account.logout(relay); } catch {}
        api.account.setToken(null);
        unlock.signOut();
        toast('Signed out', 'ok');
        navigate('#/login');
      };
    }).catch(e => {
      if (e.status === 401) {
        toast('Account session expired — sign in again', 'error');
        signedOut();
        return;
      }
      app.innerHTML = `<div class="error-panel"><p>${esc(e.message)}</p></div>`;
    });
  }

  function passphrase(vaultId, label, opts = {}) {
    app.innerHTML = `
      <div class="page">
        <div class="panel" style="max-width:34rem;margin:2rem auto;">
          <div class="panel-header">Unlock ${esc(label || vaultId)}${label && label !== vaultId ? ` <span class="mono">${esc(vaultId)}</span>` : ''}</div>
          <div class="unlock-form">
            <p class="faint" style="margin:0 0 0.6rem;">Enter this vault's passphrase — the one engramd created when the vault was set up. It is NOT your account passkey. Used only in this tab, never saved.</p>
            <input id="unlock-pass" type="password" placeholder="Vault passphrase" autocomplete="off" autocapitalize="off" spellcheck="false">
            <div id="unlock-progress"></div>
            <div class="mutation">
              <button class="btn btn-primary" id="unlock-go">Unlock</button>
              <button class="btn" id="unlock-back">Back</button>
            </div>
          </div>
        </div>
      </div>`;
    const progress = document.getElementById('unlock-progress');
    const go = async () => {
      const pass = document.getElementById('unlock-pass').value;
      if (!pass) return;
      const btn = document.getElementById('unlock-go');
      btn.disabled = true;
      try {
        await unlock.unlock(relay, vaultId, pass, (p) => {
          if (p.stage === 'pulling') progress.innerHTML = `<div class="faint">Downloading encrypted blobs… ${p.fetched}</div>`;
          else if (p.stage === 'deriving') progress.innerHTML = `<div class="faint">Deriving keys… (${p.step}/2) — a few seconds</div>`;
          else if (p.stage === 'verifying') progress.innerHTML = `<div class="faint">Verifying ${p.done}/${p.total}…</div>`;
        });
        if (opts.openByDefault) {
          try { await makeOpenByDefault(vaultId); }
          catch (e) { toast(e.message, 'error'); }
        }
        list(!!opts.openByDefault);
      } catch (e) {
        if (e.status === 401) {
          toast('Account session expired — sign in again', 'error');
          signedOut();
          return;
        }
        progress.innerHTML = `<div class="error-panel"><p>${esc(e.message)}</p></div>`;
        btn.disabled = false;
      }
    };
    document.getElementById('unlock-go').onclick = go;
    document.getElementById('unlock-pass').onkeydown = (e) => { if (e.key === 'Enter') go(); };
    document.getElementById('unlock-pass').focus();
    document.getElementById('unlock-back').onclick = () => picker();
  }

  function detail(m) {
    app.insertAdjacentHTML('beforeend', `
      <div class="modal-overlay" id="unlock-detail">
        <div class="modal detail-modal">
          <div class="detail-card">
            <div class="detail-header">
              ${layerIcon(m.layer)} <span class="detail-layer">${(m.layer || '').toUpperCase()} MEMORY</span>
              ${strengthBar(m.strength || 0)}
              <span class="faint ml-auto mono">${esc(m.id)}</span>
            </div>
            <div class="detail-content">${esc(m.content || '')}</div>
            <div class="detail-meta">
              <div class="meta-row"><span class="faint">Valence</span> ${valenceLabel(m.valence || 0)}</div>
              <div class="meta-row"><span class="faint">Created</span> ${m.created_at ? new Date(m.created_at).toLocaleString() : '?'} · ${ago(m.created_at)}</div>
              ${m.modified_at ? `<div class="meta-row"><span class="faint">Modified</span> ${new Date(m.modified_at).toLocaleString()}</div>` : ''}
              <div class="meta-row"><span class="faint">Last retrieved</span> ${m.last_retrieved ? ago(m.last_retrieved) : 'never'}</div>
              <div class="meta-row"><span class="faint">Retrievals</span> ${m.retrievals || 0}</div>
              ${m.occurred_at ? `<div class="meta-row"><span class="faint">Occurred</span> ${new Date(m.occurred_at).toLocaleString()}</div>` : ''}
              <div class="meta-row"><span class="faint">Source</span> ${sourceIcon(m.source)} ${m.source || '?'}</div>
              <div class="meta-row"><span class="faint">Project</span> ${m.project || '—'}</div>
              ${m.scope ? `<div class="meta-row"><span class="faint">Scope</span> ${esc(m.scope)}</div>` : ''}
              ${m.privacy_level ? `<div class="meta-row"><span class="faint">Privacy</span> ${esc(m.privacy_level)}</div>` : ''}
              ${m.content_type ? `<div class="meta-row"><span class="faint">Type</span> ${esc(m.content_type)}</div>` : ''}
              ${m.context ? `<div class="meta-row"><span class="faint">Context</span> ${esc(String(m.context).slice(0, 300))}</div>` : ''}
              <div class="meta-row">${tagList(m.tags)}</div>
            </div>
          </div>
          <div class="mutation">
            <button class="btn" id="unlock-detail-close">Close</button>
          </div>
        </div>
      </div>`);
    const overlay = document.getElementById('unlock-detail');
    document.getElementById('unlock-detail-close').onclick = () => overlay.remove();
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
  }

  function list(isOpen = false) {
    const meta = unlock.getMeta();
    const policy = localStorage.getItem('engram-lock-policy:' + meta.vaultId) || 'session';
    const memories = unlock.getMemories();
    const skipped = meta.hmacFailed + meta.corrupt;
    app.innerHTML = `
      <div class="page">
        <div class="unlock-header">
          <span class="mono">${esc(meta.vaultId)}</span>
          <span class="faint">${meta.memoryCount} memories · ${meta.verified} verified${skipped ? ` · ${skipped} skipped` : ''}</span>
          <span class="unlock-header-actions">
            <select class="btn btn-sm" id="unlock-policy" title="Lock policy for this vault">
              <option value="session"${policy === 'session' ? ' selected' : ''}>Open while signed in</option>
              <option value="close"${policy === 'close' ? ' selected' : ''}>Lock when I leave</option>
              <option value="5"${policy === '5' ? ' selected' : ''}>Lock after 5 min</option>
              <option value="15"${policy === '15' ? ' selected' : ''}>Lock after 15 min</option>
              <option value="60"${policy === '60' ? ' selected' : ''}>Lock after 60 min</option>
            </select>
            <button class="btn btn-sm" id="unlock-lock">Lock</button>
          </span>
        </div>
        ${skipped > 0 ? `
          <div class="unlock-warning">
            ${skipped} blob${skipped === 1 ? '' : 's'} failed integrity checks and ${skipped === 1 ? 'was' : 'were'} skipped.
            This can mean an old client wrote a different blob format — they stay out of view, nothing else is affected.
          </div>` : ''}
        <div id="unlock-list" class="unlock-list">
          ${memories.map(m => `
            <div class="memory-card" data-unlock-id="${esc(m.id)}">
              <div class="card-top">
                ${layerIcon(m.layer)}
                <span class="card-source">${sourceIcon(m.source)} ${m.source || '?'}</span>
                ${strengthBar(m.strength || 0)}
                <span class="faint ml-auto">${ago(m.created_at)}</span>
              </div>
              <div class="card-content">${esc((m.content || '').slice(0, 200))}${(m.content || '').length > 200 ? '…' : ''}</div>
              <div class="card-footer">
                ${tagList(m.tags)}
                ${m.links && m.links.length ? `<span class="faint">→ ${m.links.length} links</span>` : ''}
                ${m.imagined && !m.grounded ? '<span class="badge badge-quarantined">⚠ quarantined</span>' : ''}
              </div>
            </div>
          `).join('')}
        </div>
      </div>`;
    document.getElementById('unlock-list').addEventListener('click', (e) => {
      const card = e.target.closest('[data-unlock-id]');
      if (card) detail(unlock.getMemory(card.getAttribute('data-unlock-id')));
    });
    document.getElementById('unlock-lock').onclick = () => {
      unlock.lock();
      picker();
    };
    updateStatus();

    // Per-vault lock policy (client-side only; stored per vault_id).
    const polKey = 'engram-lock-policy:' + meta.vaultId;
    const polSel = document.getElementById('unlock-policy');
    polSel.onchange = () => {
      localStorage.setItem(polKey, polSel.value);
      if (parseInt(polSel.value, 10)) onActivity();
      else if (idleTimer) { clearTimeout(idleTimer); idleTimer = null; }
    };
    if (parseInt(polSel.value, 10)) onActivity();

    // Locked-vault path: offer to make it open by default (K is in memory).
    if (!isOpen) {
      const actions = document.querySelector('.unlock-header-actions');
      const btn = document.createElement('button');
      btn.className = 'btn btn-sm';
      btn.id = 'unlock-open-default';
      btn.textContent = 'Open by default';
      btn.title = "Wrap this vault's keys under your account key — it opens without the passphrase while signed in";
      btn.onclick = async () => {
        btn.disabled = true;
        try { await makeOpenByDefault(meta.vaultId); btn.remove(); }
        catch (e) { btn.disabled = false; toast(e.message, 'error'); }
      };
      actions.insertBefore(btn, polSel);
    }
  }

  if (unlock.isUnlocked()) { list(); return; }
  relay = await relayUrl();
  if (!api.account.token()) { signedOut(); return; }
  picker();
  // Route cleanup: the "lock when I leave" policy locks the vault on
  // navigation away; the inactivity timer dies with the route.
  return () => {
    const vaultId = unlock.getMeta() && unlock.getMeta().vaultId;
    if (vaultId && localStorage.getItem('engram-lock-policy:' + vaultId) === 'close') unlock.lock();
    if (idleTimer) { clearTimeout(idleTimer); idleTimer = null; }
  };
});

// ── Dashboard ─────────────────────────────────────────────────────────────

route('/', async () => {
  let stats, health, activity, co2, history;
  try { stats = await api.stats(); } catch (e) { stats = null; }

  const app = document.getElementById('app');

  if (!stats) {
    if (!hasAuth()) return;  // signed out mid-load — the login screen owns #app
    app.innerHTML = `<div class="error-panel">
      <h2>Cannot reach engramd</h2>
      <p>Make sure <code>engramd</code> is running on port 8787.</p>
    </div>`;
    return;
  }

  try { health = await api.health(); } catch (e) { health = null; }
  try { activity = await api.analytics.activity(30); } catch (e) { activity = null; }
  try { co2 = await api.analytics.co2(); } catch (e) { co2 = null; }
  try { history = await api.consolidate.history(); } catch (e) { history = []; }
  const runs = Array.isArray(history) ? history : (history.runs || []);

  // One recency-sorted sample drives the at-risk count, histogram and initial feed
  let sample = [];
  try {
    const r = await api.memories.search({ sort_by: 'recency', limit: 200 });
    sample = Array.isArray(r) ? r : (r.results || []);
  } catch (e) { /* feed stays empty */ }

  const total = stats.total ?? stats.total_memories ?? 0;
  const layerPct = (n) => total ? ((n / total) * 100).toFixed(1) : 0;

  // /analytics/activity returns { activity: [{day, count}], days: N } —
  // per-day tokens/CO2 are not tracked, so the sparks show capture counts.
  const days = Array.isArray(activity?.activity) ? activity.activity : [];
  const tokensSaved = co2?.estimated_tokens_saved || 0;
  const tokensSpark = days.map(d => d.count || 0);
  const co2TotalG = co2?.estimated_co2_grams || 0;
  const co2Spark = []; // no per-day CO2 metric — card shows dedupe savings instead

  const atRisk = sample.filter(m => (m.strength ?? 1) < 0.3).length;

  // Strength histogram buckets: 0–0.5, 0.5–1.0, 1.0–1.5, 1.5–2.0
  const bucketLabels = ['0–0.5', '0.5–1.0', '1.0–1.5', '1.5–2.0'];
  const buckets = [0, 0, 0, 0];
  for (const m of sample) buckets[Math.max(0, Math.min(3, Math.floor((m.strength ?? 0) / 0.5)))]++;
  const maxBucket = Math.max(1, ...buckets);

  // Decay forecast: last decay run's decayed count
  const lastDecay = runs.find(r => r.type === 'decay');
  const forecast = lastDecay ? (lastDecay.engrams_decayed || 0) : 0;

  app.innerHTML = `
    <div class="page dashboard">
      <div class="stat-grid">
        <div class="stat-card">
          <div class="stat-num">${total.toLocaleString()}</div>
          <div class="stat-label">Total memories</div>
          <div class="stat-sub">${qemStatus(health)} · ${formatBytes(health?.db_size_bytes || stats.db_size_bytes || 0)}</div>
        </div>
        <div class="stat-card">
          <div class="stat-num">${tokensSaved.toLocaleString()}</div>
          <div class="stat-label">Tokens saved (est.)</div>
          ${sparkline(tokensSpark)}
        </div>
        <div class="stat-card">
          <div class="stat-num">${(co2TotalG / 1000).toFixed(1)}<span class="stat-unit">kg</span></div>
          <div class="stat-label">CO₂ avoided (cumulative)</div>
          <div class="stat-sub">${(co2?.deduped_saves || 0).toLocaleString()} duplicate saves · ${(co2?.noise_skips || 0).toLocaleString()} noise skips</div>
        </div>
        <div class="stat-card">
          <div class="stat-num">${atRisk}</div>
          <div class="stat-label">At risk of decay</div>
          <div class="stat-sub">strength &lt; 0.3</div>
        </div>
      </div>

      <div class="dashboard-grid">
        <div class="panel live-panel">
          <div class="panel-header">Live Feed <span id="live-status" class="live-status reconnecting">○ connecting…</span></div>
          <div class="quick-capture">
            <textarea id="qc-input" rows="2" placeholder="Quick capture — what's happening?"></textarea>
            <button id="qc-btn" class="btn btn-primary">Capture</button>
          </div>
          <div id="live-feed" class="live-feed"></div>
        </div>

        <div class="dashboard-side">
          <div class="panel">
            <div class="panel-header">Layer Breakdown</div>
            <div class="layer-breakdown">
              <div class="layer-row">
                <span>${layerIcon('episodic')} Episodic</span>
                <span class="faint">${stats.by_layer?.episodic || 0} · ${layerPct(stats.by_layer?.episodic || 0)}%</span>
              </div>
              <div class="layer-bar"><div class="layer-bar-fill episodic" style="width:${layerPct(stats.by_layer?.episodic || 0)}%"></div></div>
              <div class="layer-row">
                <span>${layerIcon('semantic')} Semantic</span>
                <span class="faint">${stats.by_layer?.semantic || 0} · ${layerPct(stats.by_layer?.semantic || 0)}%</span>
              </div>
              <div class="layer-bar"><div class="layer-bar-fill semantic" style="width:${layerPct(stats.by_layer?.semantic || 0)}%"></div></div>
              <div class="layer-row">
                <span>${layerIcon('imagined')} Imagined</span>
                <span class="faint">${stats.by_layer?.imagined || 0} · ${layerPct(stats.by_layer?.imagined || 0)}%</span>
              </div>
              <div class="layer-bar"><div class="layer-bar-fill imagined" style="width:${layerPct(stats.by_layer?.imagined || 0)}%"></div></div>

              <div class="histogram">
                <div class="hist-title faint">Strength distribution</div>
                ${buckets.map((c, bi) => `
                  <div class="hist-row">
                    <span class="hist-label">${bucketLabels[bi]}</span>
                    <div class="hist-bar"><div class="hist-fill h${bi}" style="width:${(c / maxBucket) * 100}%"></div></div>
                    <span class="hist-count">${c}</span>
                  </div>
                `).join('')}
              </div>
            </div>
          </div>

          <div class="panel">
            <div class="panel-header">Decay Forecast</div>
            <div class="decay-forecast">
              <span class="decay-icon">↓</span>
              <div>
                <div class="stat-num">${forecast}</div>
                <div class="stat-label">memories will fall below the retrieval threshold this week</div>
                <div class="stat-sub">${lastDecay ? 'based on the last decay run · ' + ago(lastDecay.run_at) : 'no decay runs recorded yet'}</div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  `;

  // ── Live feed wiring ────────────────────────────────────────────────────
  const feedEl = document.getElementById('live-feed');
  const seen = new Set();
  const MAX_FEED = 30;

  function addToFeed(m, animate) {
    if (!m || !m.id || seen.has(m.id)) return;
    seen.add(m.id);
    feedEl.querySelector(':scope > .faint')?.remove(); // drop placeholder
    const tmp = document.createElement('div');
    tmp.innerHTML = liveFeedItem(m);
    const el = tmp.firstElementChild;
    if (animate) el.classList.add('slide-in');
    feedEl.prepend(el);
    while (feedEl.children.length > MAX_FEED) feedEl.lastElementChild.remove();
  }

  if (sample.length) {
    for (let i = Math.min(sample.length, 10) - 1; i >= 0; i--) addToFeed(sample[i], false);
  } else {
    feedEl.innerHTML = '<div class="faint" style="padding:1rem;">No memories captured yet.</div>';
  }

  const statusEl = document.getElementById('live-status');
  function setLiveStatus(s) {
    statusEl.className = 'live-status ' + s;
    statusEl.textContent = s === 'live' ? '● live'
      : s === 'polling' ? '◐ polling · reconnecting…'
      : '○ reconnecting…';
  }

  const stream = connectEventStream({
    onEvent: (ev) => { if (ev.type === 'capture' && ev.memory) addToFeed(ev.memory, true); },
    onStatus: setLiveStatus,
  });

  // ── Quick capture ───────────────────────────────────────────────────────
  document.getElementById('qc-btn').onclick = async () => {
    const ta = document.getElementById('qc-input');
    const content = ta.value.trim();
    if (!content) return;
    try {
      const r = await captureMemory({ content });
      if (r === null) return; // skipped — toast shown, keep the text
      ta.value = '';
      toast('Captured', 'ok');
      // Pull the fresh memory into the feed (WS will dedupe by id)
      const res = await api.memories.search({ sort_by: 'recency', limit: 3 });
      const list = Array.isArray(res) ? res : (res.results || []);
      for (let i = list.length - 1; i >= 0; i--) addToFeed(list[i], true);
    } catch (e) {
      toast('Capture failed: ' + e.message, 'error');
    }
  };
  document.getElementById('qc-input').onkeydown = (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) document.getElementById('qc-btn').click();
  };

  updateStatus();
  return () => stream.close();
});

// ── Stats ─────────────────────────────────────────────────────────────────

route('/stats', async () => {
  const app = document.getElementById('app');
  let co2, activity, stats;
  try { co2 = await api.analytics.co2(); } catch (e) { co2 = null; }
  try { activity = await api.analytics.activity(30); } catch (e) { activity = null; }
  try { stats = await api.stats(); } catch (e) { stats = null; }

  // /analytics/activity returns { activity: [{day, count}], days: N } — count
  // is captures; per-day tokens/CO2/retrievals are not tracked by the API.
  const days = (Array.isArray(activity?.activity) ? activity.activity : [])
    .map(d => ({ day: d.day, captures: d.count || 0, retrievals: 0 }));
  const co2G = co2?.estimated_co2_grams || 0;
  const tokensTotal = co2?.estimated_tokens_saved || 0;
  const co2Kg = co2G / 1000;
  const miles = co2G / 404; // 404 g CO₂ per driving mile
  const co2Spark = []; // no per-day CO2 metric

  // Dedup impact: duplicates prevented × avg tokens saved per capture
  const deduped = co2?.deduped_saves || 0;
  const captures = co2?.total_captures || 0;
  const perCapture = captures ? tokensTotal / captures : 0;
  const dedupTokens = Math.round(deduped * perCapture);

  app.innerHTML = `
    <div class="page stats-page">
      <h2>Vault Impact</h2>

      <div class="panel co2-hero" style="margin-bottom:1rem;">
        <div class="panel-header">CO₂ Avoided</div>
        <div class="co2-hero-body">
          <div>
            <div class="stat-num big">${co2Kg.toFixed(1)}<span class="stat-unit">kg</span></div>
            <div class="stat-label">cumulative CO₂ saved by token compression</div>
            <div class="stat-sub">Equivalent to driving ${miles.toFixed(1)} miles (404 g/mile)</div>
          </div>
          ${sparkline(co2Spark, 180, 48)}
        </div>
      </div>

      <div class="stat-grid" style="grid-template-columns:repeat(3,1fr);margin-bottom:1rem;">
        <div class="stat-card">
          <div class="stat-num">${tokensTotal.toLocaleString()}</div>
          <div class="stat-label">Tokens saved (cumulative)</div>
        </div>
        <div class="stat-card">
          <div class="stat-num">${captures.toLocaleString()}</div>
          <div class="stat-label">Captures</div>
          <div class="stat-sub">${(co2?.total_retrievals || 0).toLocaleString()} retrievals</div>
        </div>
        <div class="stat-card">
          <div class="stat-num">${stats?.qem_hit_rate ? (stats.qem_hit_rate * 100).toFixed(1) + '%' : '—'}</div>
          <div class="stat-label">QEM hit rate</div>
          <div class="stat-sub">holographic L1 cache</div>
        </div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Memory Activity — Last 30 Days</div>
        <div class="chart-wrap">
          ${activityChart(days)}
          <div class="chart-legend">
            <span><span class="lg-dot" style="background:var(--episodic)"></span> Captures</span>
          </div>
        </div>
      </div>

      <div class="panel-grid">
        <div class="panel">
          <div class="panel-header">Layer Distribution</div>
          <div class="chart-wrap">${layerDistribution(stats?.by_layer)}</div>
        </div>
        <div class="panel">
          <div class="panel-header">Deduplication Impact</div>
          <div class="dedup-body">
            <div class="stat-num">${deduped.toLocaleString()}</div>
            <div class="stat-label">duplicates prevented</div>
            <div class="stat-sub">≈ ${dedupTokens.toLocaleString()} tokens saved by dedup alone</div>
          </div>
        </div>
      </div>
    </div>
  `;

  updateStatus();
});

// ── Weekly digest ─────────────────────────────────────────────────────────

route('/digest', async () => {
  const app = document.getElementById('app');
  let days = 7;
  let lastData = null;

  const render = (d) => {
    lastData = d;
    const s = d.stats || {};
    const memList = (items) => (items || []).map(m => `
      <li class="digest-mem">
        <span class="digest-mem-head">
          <span class="digest-tag">${esc(m.layer || '')}</span>
          ${m.tags && m.tags.length ? `<span class="digest-tags">${m.tags.map(esc).join(' · ')}</span>` : ''}
        </span>
        <span class="digest-content">${esc(m.content)}</span>
      </li>`).join('');
    const themes = (d.themes || []).map(t => `
      <div class="digest-theme">
        <div class="digest-theme-head">
          <strong>${esc(t.label)}</strong>
          <span class="digest-count">${t.count} memories</span>
        </div>
        <ul>${(t.examples || []).map(e => `<li>${esc(e)}</li>`).join('')}</ul>
      </div>`).join('');

    app.innerHTML = `
      <div class="page digest-page">
        <h2>Weekly Digest</h2>
        <p class="digest-sub">What your AI learned about you — assembled locally, nothing leaves your machine.</p>

        <div class="digest-controls">
          <div class="btn-group">
            ${[7, 30].map(n => `<button class="btn btn-sm ${n === days ? 'btn-primary' : ''}" data-days="${n}">${n} days</button>`).join('')}
          </div>
          <span class="digest-window">${esc((d.window_start || '').slice(0, 10))} → ${esc((d.window_end || '').slice(0, 10))}</span>
        </div>

        <div class="stat-grid" style="grid-template-columns:repeat(3,1fr);margin-bottom:1rem;">
          <div class="stat-card"><div class="stat-num">${s.live_total ?? 0}</div><div class="stat-label">Live memories</div></div>
          <div class="stat-card"><div class="stat-num">${s.new ?? 0}</div><div class="stat-label">New this week</div></div>
          <div class="stat-card"><div class="stat-num">${s.reinforced ?? 0}</div><div class="stat-label">Reinforced by use</div></div>
          <div class="stat-card"><div class="stat-num">${s.fading ?? 0}</div><div class="stat-label">Fading</div></div>
          <div class="stat-card"><div class="stat-num">${s.quarantined ?? 0}</div><div class="stat-label">Quarantined</div></div>
          <div class="stat-card"><div class="stat-num">${s.quarantined_new ?? 0}</div><div class="stat-label">Noise filtered</div></div>
        </div>

        ${themes ? `<div class="panel" style="margin-bottom:1rem;"><div class="panel-header">Themes</div><div class="digest-themes">${themes}</div></div>` : ''}

        <div class="panel-grid">
          <div class="panel">
            <div class="panel-header">New memories</div>
            ${(d.new_memories || []).length ? `<ul class="digest-list">${memList(d.new_memories)}</ul>` : '<div class="empty-state">Nothing new this window.</div>'}
          </div>
          <div class="panel">
            <div class="panel-header">Reinforced (used this week)</div>
            ${(d.reinforced || []).length ? `<ul class="digest-list">${memList(d.reinforced)}</ul>` : '<div class="empty-state">Nothing re-used this window.</div>'}
          </div>
          <div class="panel">
            <div class="panel-header">Fading (worth revisiting)</div>
            ${(d.fading || []).length ? `<ul class="digest-list">${memList(d.fading)}</ul>` : '<div class="empty-state">Nothing fading this window.</div>'}
          </div>
        </div>

        <div class="panel" style="margin-top:1rem;">
          <div class="panel-header">Prose digest</div>
          ${d.prose
            ? `<div class="digest-prose">${d.prose.split('\n').filter(p => p.trim()).map(p => `<p>${esc(p)}</p>`).join('')}</div>`
            : `<div class="digest-prose-cta">
                 ${d.llm_configured
                   ? '<p>Narrative summary written by your own LLM — the call uses your BYO key and bills you, so it runs only when you ask.</p><button class="btn btn-primary" id="digest-prose-btn">Generate prose</button>'
                   : '<p>No LLM configured. Add a BYO-key endpoint (or a local Ollama) in <a href="#/settings">Settings</a> to get an AI-written narrative.</p>'}
               </div>`}
        </div>
      </div>`;

    app.querySelectorAll('[data-days]').forEach(btn => {
      btn.addEventListener('click', async () => {
        days = parseInt(btn.dataset.days, 10);
        app.innerHTML = '<div class="loading">Loading…</div>';
        try {
          render(await api.digest.weekly(days, false));
        } catch (e) {
          toast(e.message, 'warn');
          if (lastData) render(lastData);
        }
      });
    });
    const proseBtn = app.querySelector('#digest-prose-btn');
    if (proseBtn) {
      proseBtn.addEventListener('click', async () => {
        proseBtn.disabled = true;
        proseBtn.textContent = 'Generating…';
        try {
          render(await api.digest.weekly(days, true));
        } catch (e) {
          toast(e.message, 'warn');
          proseBtn.disabled = false;
          proseBtn.textContent = 'Generate prose';
        }
      });
    }
  };

  try {
    render(await api.digest.weekly(days, false));
  } catch (e) {
    app.innerHTML = `
      <div class="page">
        <h2>Weekly Digest</h2>
        <div class="panel"><div class="empty-state">Could not load the digest: ${esc(e.message)}</div></div>
      </div>`;
  }
  updateStatus();
});

// ── Tracker (live capture stream) ─────────────────────────────────────────

route('/tracker', async () => {
  const app = document.getElementById('app');
  const SOURCES = ['observation', 'interaction', 'agent', 'chat', 'window', 'system', 'consolidation', 'research', 'mic', 'sensor', 'imagined'];

  app.innerHTML = `
    <div class="page tracker-page">
      <div class="filter-row">
        <select id="tr-source" class="filter-select">
          <option value="">All sources</option>
          ${SOURCES.map(s => `<option value="${s}">${s}</option>`).join('')}
        </select>
        <select id="tr-layer" class="filter-select">
          <option value="">All layers</option>
          <option value="episodic">Episodic</option>
          <option value="semantic">Semantic</option>
          <option value="imagined">Imagined</option>
        </select>
        <button id="tr-pause" class="btn btn-sm">⏸ Pause</button>
        <span id="tr-status" class="live-status reconnecting">○ connecting…</span>
        <span class="faint ml-auto" id="tr-count"></span>
      </div>
      <div id="tracker-feed" class="tracker-feed"></div>
    </div>
  `;

  const feedEl = document.getElementById('tracker-feed');
  const pauseBtn = document.getElementById('tr-pause');
  const countEl = document.getElementById('tr-count');
  const seen = new Set();
  const buffer = [];
  let paused = false;
  const MAX_ITEMS = 100;

  function trackerItem(m) {
    const content = m.content || '';
    return `
      <div class="tracker-item" data-id="${esc(m.id)}" data-source="${esc(m.source || '')}" data-layer="${esc(m.layer || '')}">
        <div class="ti-head">
          ${layerIcon(m.layer)}
          <span class="ti-source">${sourceIcon(m.source)} ${esc(m.source || '?')}</span>
          ${tagList((m.tags || []).slice(0, 4))}
          ${notesBadge(m)}
          <span class="faint ml-auto">${ago(m.created_at)}</span>
        </div>
        <div class="ti-preview">${esc(content.slice(0, 140))}${content.length > 140 ? '…' : ''}</div>
        <div class="ti-full">
          <div class="ti-full-content">${esc(content)}</div>
          <a href="#/memories/${m.id}" class="ti-open">Open detail →</a>
        </div>
      </div>`;
  }

  function applyFilters() {
    const fs = document.getElementById('tr-source').value;
    const fl = document.getElementById('tr-layer').value;
    let visible = 0;
    feedEl.querySelectorAll('.tracker-item').forEach(el => {
      const ok = (!fs || el.dataset.source === fs) && (!fl || el.dataset.layer === fl);
      el.classList.toggle('hidden', !ok);
      if (ok) visible++;
    });
    countEl.textContent = `${visible} shown · ${seen.size} total`;
  }

  function addItem(m, animate) {
    if (!m || !m.id || seen.has(m.id)) return false;
    seen.add(m.id);
    feedEl.querySelector(':scope > .faint')?.remove(); // drop placeholder
    const tmp = document.createElement('div');
    tmp.innerHTML = trackerItem(m);
    const el = tmp.firstElementChild;
    if (animate) el.classList.add('slide-in');
    feedEl.prepend(el);
    while (feedEl.children.length > MAX_ITEMS) {
      const last = feedEl.lastElementChild;
      seen.delete(last.dataset.id);
      last.remove();
    }
    return true;
  }

  function onEvent(ev) {
    if (ev.type !== 'capture' || !ev.memory) return;
    if (paused) {
      if (!seen.has(ev.memory.id) && !buffer.some(b => b.id === ev.memory.id)) buffer.push(ev.memory);
      pauseBtn.innerHTML = `▶ Resume <span class="pause-badge">${buffer.length} new</span>`;
      return;
    }
    if (addItem(ev.memory, true)) applyFilters();
  }

  // Pause / resume
  pauseBtn.onclick = () => {
    paused = !paused;
    if (paused) {
      pauseBtn.classList.add('paused');
      pauseBtn.textContent = '▶ Resume';
    } else {
      pauseBtn.classList.remove('paused');
      pauseBtn.textContent = '⏸ Pause';
      // Flush buffered items, oldest first so newest lands on top
      for (const m of buffer) addItem(m, true);
      buffer.length = 0;
      applyFilters();
    }
  };

  document.getElementById('tr-source').onchange = applyFilters;
  document.getElementById('tr-layer').onchange = applyFilters;

  // Click to expand / collapse
  feedEl.addEventListener('click', (e) => {
    if (e.target.closest('a')) return;
    const item = e.target.closest('.tracker-item');
    if (item) item.classList.toggle('expanded');
  });

  const statusEl = document.getElementById('tr-status');
  function setStatus(s) {
    statusEl.className = 'live-status ' + s;
    statusEl.textContent = s === 'live' ? '● live'
      : s === 'polling' ? '◐ polling · reconnecting…'
      : '○ reconnecting…';
  }

  // Initial fill
  try {
    const r = await api.memories.search({ sort_by: 'recency', limit: 30 });
    const list = Array.isArray(r) ? r : (r.results || []);
    for (let i = list.length - 1; i >= 0; i--) addItem(list[i], false);
  } catch (e) { /* stream will fill in */ }
  if (!feedEl.children.length) {
    feedEl.innerHTML = '<div class="faint" style="padding:1rem;">Waiting for captures…</div>';
  }
  applyFilters();

  const stream = connectEventStream({ onEvent, onStatus: setStatus });

  updateStatus();
  return () => stream.close();
});

// ── Explorer ──────────────────────────────────────────────────────────────

route('/memories', async () => {
  const app = document.getElementById('app');
  app.innerHTML = `
    <div class="page explorer">
      <div class="search-bar">
        <input type="text" id="search-input" class="search-input" placeholder="Search memories…" autofocus>
        <button id="search-btn" class="btn btn-primary">Search</button>
      </div>
      <div class="filter-row">
        <select id="filter-layer" class="filter-select">
          <option value="">All layers</option>
          <option value="episodic">Episodic</option>
          <option value="semantic">Semantic</option>
          <option value="imagined">Imagined</option>
        </select>
        <select id="filter-sort" class="filter-select">
          <option value="relevance">Relevance</option>
          <option value="strength">Strength</option>
          <option value="recency">Recency</option>
          <option value="valence">Valence</option>
        </select>
        <button id="filter-quarantined" class="pill-toggle" title="Show only quarantined memories (imagined, not yet grounded)">⚠ Quarantined</button>
        <span class="faint" style="margin-left:auto;" id="result-count"></span>
      </div>
      <div id="explorer-results" class="explorer-results"></div>
    </div>
  `;

  function doSearch() {
    const q = document.getElementById('search-input').value;
    const layer = document.getElementById('filter-layer').value;
    const sort = document.getElementById('filter-sort').value;
    const qonly = document.getElementById('filter-quarantined').classList.contains('active');
    const results = document.getElementById('explorer-results');
    results.innerHTML = '<div class="loading-sm">Searching…</div>';
    api.memories.search({ query: q, layer: layer || null, quarantined: qonly ? true : null, limit: 20 }).then(r => {
      const list = Array.isArray(r) ? r : (r.results || []);
      document.getElementById('result-count').textContent = `${list.length} results`;
      if (!list.length) {
        results.innerHTML = '<div class="faint" style="padding:1rem;">No results.</div>';
        return;
      }
      results.innerHTML = list.map(m => `
        <a href="#/memories/${m.id}" class="memory-card">
          <div class="card-top">
            ${layerIcon(m.layer)}
            <span class="card-source">${sourceIcon(m.source)} ${m.source || '?'}</span>
            ${strengthBar(m.strength || 0)}
            <span class="faint ml-auto">${ago(m.created_at)}</span>
          </div>
          <div class="card-content">${esc((m.content || '').slice(0, 200))}${(m.content || '').length > 200 ? '…' : ''}</div>
          <div class="card-footer">
            ${tagList(m.tags)}
            ${m.links && m.links.length ? `<span class="faint">→ ${m.links.length} links</span>` : ''}
            ${notesBadge(m)}
            ${m.imagined && !m.grounded ? '<span class="badge badge-quarantined">⚠ quarantined</span>' : ''}
            ${!m.imagined || m.grounded ? `<button class="btn btn-sm" data-action="mark-noise" data-id="${m.id}">Mark noise</button>` : ''}
          </div>
        </a>
      `).join('');
    }).catch(e => {
      results.innerHTML = `<div class="error">Search failed: ${esc(e.message)}</div>`;
    });
  }

  document.getElementById('search-btn').onclick = doSearch;
  document.getElementById('search-input').onkeydown = (e) => { if (e.key === 'Enter') doSearch(); };
  document.getElementById('filter-layer').onchange = doSearch;
  document.getElementById('filter-sort').onchange = doSearch;
  document.getElementById('filter-quarantined').onclick = () => {
    document.getElementById('filter-quarantined').classList.toggle('active');
    doSearch();
  };

  // CSP note: no inline handlers — mark-noise clicks are delegated here so
  // the card anchor navigation doesn't fire.
  document.getElementById('explorer-results').addEventListener('click', async (e) => {
    const btn = e.target.closest('[data-action="mark-noise"]');
    if (!btn) return;
    e.preventDefault();
    e.stopPropagation();
    await api.memories.markNoise(btn.getAttribute('data-id'));
    toast('Marked as noise — moved to quarantine', 'ok');
    doSearch();
  });

  doSearch();
  updateStatus();
});

// ── Detail ────────────────────────────────────────────────────────────────

route('/memories/:id', async (id) => {
  const app = document.getElementById('app');
  app.innerHTML = '<div class="loading">Loading memory…</div>';

  try {
    const m = await api.memories.get(id);
    let linksHtml = '';
    try {
      const l = await api.memories.links(id);
      const linkList = Array.isArray(l) ? l : (l.outgoing || l.incoming || []);
      if (linkList.length) {
        linksHtml = `
          <div class="panel" style="margin-top:1rem;">
            <div class="panel-header">Links (${linkList.length} outgoing)</div>
            ${linkList.map(ln => `
              <a href="#/memories/${ln.target_id}" class="feed-item">
                ${ln.link_type === 'causal' ? '▶' : ln.link_type === 'associative' ? '··' : ln.link_type === 'analogical' ? '--' : '··>'}
                <span class="faint">${ln.link_type}</span>
                <span>${esc(ln.target_id)}</span>
                <span class="faint ml-auto">weight ${ln.weight.toFixed(2)}</span>
              </a>
            `).join('')}
          </div>`;
      }
    } catch (e) { /* no links */ }

    app.innerHTML = `
      <div class="page detail">
        <a href="#/memories" class="back-link">← Back to Explorer</a>
        <div class="detail-card">
          <div class="detail-header">
            ${layerIcon(m.layer)} <span class="detail-layer">${m.layer?.toUpperCase()} MEMORY</span>
            ${strengthBar(m.strength || 0)}
            <span class="faint ml-auto">${m.id}</span>
          </div>
          <div class="detail-content">${esc(m.content || '')}</div>
          <div class="detail-meta">
            <div class="meta-row"><span class="faint">Valence</span> ${valenceLabel(m.valence || 0)}</div>
            <div class="meta-row"><span class="faint">Created</span> ${m.created_at ? new Date(m.created_at).toLocaleString() : '?'} · ${ago(m.created_at)}</div>
            <div class="meta-row"><span class="faint">Last retrieved</span> ${m.last_retrieved ? ago(m.last_retrieved) : 'never'}</div>
            <div class="meta-row"><span class="faint">Retrievals</span> ${m.retrieval_count || 0}</div>
            <div class="meta-row"><span class="faint">Source</span> ${sourceIcon(m.source)} ${m.source || '?'}</div>
            <div class="meta-row"><span class="faint">Project</span> ${m.project || '—'}</div>
            <div class="meta-row">${tagList(m.tags)}</div>
          </div>
        </div>
        <div class="panel notes-panel" style="margin-top:1rem;">
          <div class="panel-header notes-header" id="notes-toggle">
            <span>Notes (<span id="notes-count">…</span>)</span>
            <span class="chev" id="notes-chev">▾</span>
          </div>
          <div id="notes-body" class="notes-body">
            <div id="notes-list" class="notes-list"><div class="loading-sm">Loading notes…</div></div>
            <div class="note-form">
              <textarea id="note-input" rows="2" placeholder="Add a note…"></textarea>
              <button id="note-save" class="btn btn-primary btn-sm">Save note</button>
            </div>
          </div>
        </div>
        ${linksHtml}
        <div class="detail-actions">
          ${m.imagined && !m.grounded
            ? '<button class="btn" id="btn-ground">Ground memory</button>'
            : '<button class="btn" id="btn-noise">Mark as noise</button>'}
          <button class="btn btn-danger" id="btn-delete">Delete</button>
        </div>
      </div>
    `;

    if (m.imagined && !m.grounded) {
      document.getElementById('btn-ground').onclick = async () => {
        await api.memories.ground(id);
        toast('Memory grounded', 'ok');
        render();
      };
    } else {
      document.getElementById('btn-noise').onclick = async () => {
        await api.memories.markNoise(id);
        toast('Marked as noise — moved to quarantine', 'ok');
        render();
      };
    }
    document.getElementById('btn-delete').onclick = async () => {
      if (confirm('Delete this memory permanently?')) {
        await api.memories.delete(id);
        toast('Deleted', 'warn');
        navigate('#/memories');
      }
    };

    // ── Notes (annotations) ───────────────────────────────────────────────
    const notesBody = document.getElementById('notes-body');
    document.getElementById('notes-toggle').onclick = () => {
      const collapsed = notesBody.classList.toggle('collapsed');
      document.getElementById('notes-chev').textContent = collapsed ? '▸' : '▾';
    };

    async function loadNotes() {
      const listEl = document.getElementById('notes-list');
      const countEl = document.getElementById('notes-count');
      try {
        const r = await api.memories.annotations(id);
        const notes = Array.isArray(r) ? r : (r.annotations || r.results || []);
        countEl.textContent = notes.length;
        if (!notes.length) {
          listEl.innerHTML = '<div class="faint" style="padding:0.5rem 0;">No notes yet.</div>';
          return;
        }
        listEl.innerHTML = notes.map(n => `
          <div class="note-item" data-id="${esc(n.id)}">
            <div class="note-content">${esc(n.content || '')}</div>
            <div class="note-meta">
              <span class="faint">${ago(n.created_at)}</span>
              <button class="note-del" data-id="${esc(n.id)}" title="Delete note">×</button>
            </div>
          </div>`).join('');
      } catch (e) {
        countEl.textContent = '0';
        listEl.innerHTML = '<div class="faint" style="padding:0.5rem 0;">Notes unavailable.</div>';
      }
    }

    document.getElementById('note-save').onclick = async () => {
      const ta = document.getElementById('note-input');
      const content = ta.value.trim();
      if (!content) return;
      try {
        await api.memories.annotate(id, content);
        ta.value = '';
        toast('Note saved', 'ok');
        loadNotes();
      } catch (e) {
        toast('Save failed: ' + e.message, 'error');
      }
    };

    document.getElementById('notes-list').addEventListener('click', async (e) => {
      const btn = e.target.closest('.note-del');
      if (!btn) return;
      await api.annotations.delete(btn.dataset.id);
      toast('Note deleted', 'warn');
      loadNotes();
    });

    loadNotes();
  } catch (e) {
    app.innerHTML = `<div class="error-panel"><h2>Not found</h2><p>Memory ${esc(id)} not found.</p><a href="#/memories">← Back</a></div>`;
  }
  updateStatus();
});

// ── Graph ─────────────────────────────────────────────────────────────────

route('/graph', async () => {
  const app = document.getElementById('app');
  app.innerHTML = `
    <div class="page graph-page">
      <div class="graph-filters">
        <select id="gf-layer" class="filter-select"><option value="">All layers</option><option value="episodic">Episodic</option><option value="semantic">Semantic</option><option value="imagined">Imagined</option></select>
        <input type="text" id="gf-search" class="search-input" placeholder="Focus on a memory ID…" style="width:220px;">
        <button class="btn btn-sm" id="gf-search-btn">Focus</button>
        <button class="btn btn-sm" id="gf-reset">Reset view</button>
        <div class="pill-row graph-pills">
          <button class="pill active pill-episodic" data-layer="episodic">Episodic</button>
          <button class="pill active pill-semantic" data-layer="semantic">Semantic</button>
          <button class="pill active pill-imagined" data-layer="imagined">Imagined</button>
        </div>
      </div>
      <div id="graph-canvas" class="graph-canvas"></div>
      <div id="graph-scrubber" class="graph-scrubber hidden">
        <span class="scrub-date" id="scrub-min-label"></span>
        <input type="range" id="scrub-min" min="0" max="1000" value="0">
        <input type="range" id="scrub-max" min="0" max="1000" value="1000">
        <span class="scrub-date" id="scrub-max-label"></span>
      </div>
      <div class="graph-legend">
        <span class="lg-item"><span class="lg-dot" style="background:#f0a040"></span> Episodic</span>
        <span class="lg-item"><span class="lg-dot" style="background:#48c0e0"></span> Semantic</span>
        <span class="lg-item"><span class="lg-dot" style="background:#b080e0"></span> Imagined</span>
        <span class="lg-sep">·</span>
        <span class="lg-item"><span class="lg-edge assoc"></span> Assoc</span>
        <span class="lg-item"><span class="lg-edge causal"></span> Causal</span>
        <span class="lg-item"><span class="lg-edge analog"></span> Analog</span>
        <span class="lg-item"><span class="lg-edge temp"></span> Temp</span>
        <span class="lg-sep">·</span>
        <span class="lg-hint">🖱 drag · scroll zoom · click expand · dblclick reset</span>
      </div>
    </div>
  `;

  const container = document.getElementById('graph-canvas');
  const graph = new MemoryGraph(container);

  // Listen for node expansion requests from the graph engine
  container.addEventListener('mg-node-expand', async (e) => {
    const { id } = e.detail;
    try {
      let relatedResp = await api.memories.related(id);
      // Handle both array and {results:[...]} response formats
      const list = Array.isArray(relatedResp) ? relatedResp : (relatedResp.results || []);
      const newNodes = list.map(m => ({
        id: m.id,
        label: (m.content || m.id).slice(0, 60),
        layer: m.layer,
        strength: m.strength || 0.5,
        valence: m.valence || 0,
        tags: m.tags || [],
        created: m.created_at,
      }));
      const newEdges = [];
      for (const n of newNodes) {
        try {
          const l = await api.memories.links(n.id);
          const outgoing = Array.isArray(l) ? l : (l.outgoing || []);
          for (const ln of outgoing) {
            if (ln.target_id === id || newNodes.find(x => x.id === ln.target_id)) {
              newEdges.push({ source: n.id, target: ln.target_id, type: ln.link_type, weight: ln.weight });
            }
          }
        } catch (_) { /* skip */ }
      }
      graph.expand(id, newNodes, newEdges);
    } catch (err) {
      toast('Expand failed: ' + esc(err.message), 'error');
    }
  });

  async function loadGraph(filterLayer, focusId) {
    try {
      const r = await api.memories.search({
        query: '',
        layer: filterLayer || null,
        limit: 50,
      });
      const searchResults = Array.isArray(r) ? r : (r.results || []);
      if (!searchResults.length) {
        container.innerHTML = '<div class="faint" style="padding:3rem;text-align:center;">No memories to graph.</div>';
        return;
      }
      const nodes = searchResults.map(m => ({
        id: m.id,
        label: (m.content || m.id).slice(0, 60),
        layer: m.layer,
        strength: m.strength || 0.5,
        valence: m.valence || 0,
        tags: m.tags || [],
        created: m.created_at,
      }));
      const edges = [];
      for (const n of nodes.slice(0, 10)) {
        try {
          const l = await api.memories.links(n.id);
          const outgoing = Array.isArray(l) ? l : (l.outgoing || []);
          if (outgoing && outgoing.length) {
            for (const ln of outgoing) {
              if (nodes.find(x => x.id === ln.target_id)) {
                edges.push({ source: n.id, target: ln.target_id, type: ln.link_type, weight: ln.weight });
              }
            }
          }
        } catch (_) { /* skip */ }
      }
      graph.load({ nodes, edges }, focusId || null);
    } catch (e) {
      container.innerHTML = `<div class="error">${esc(e.message)}</div>`;
    }
  }

  document.getElementById('gf-search-btn').onclick = () => {
    const id = document.getElementById('gf-search').value.trim();
    if (id) graph.focus(id);
  };
  document.getElementById('gf-reset').onclick = () => graph.resetView();
  document.getElementById('gf-layer').onchange = async () => {
    await loadGraph(document.getElementById('gf-layer').value, null);
    setupScrubber();
  };

  // ── Layer toggle pills ──────────────────────────────────────────────────
  document.querySelectorAll('.graph-pills .pill').forEach(p => {
    p.onclick = () => {
      const active = p.classList.toggle('active');
      graph.setLayerVisible(p.dataset.layer, active);
    };
  });

  // ── Temporal scrubber ───────────────────────────────────────────────────
  function setupScrubber() {
    const wrap = document.getElementById('graph-scrubber');
    const times = graph.nodes
      .map(n => n.created ? new Date(n.created).getTime() : NaN)
      .filter(t => !isNaN(t));
    const minT = Math.min(...times), maxT = Math.max(...times);
    const DAY = 86400000;
    // Hide when everything was captured on the same day
    if (times.length < 2 || Math.floor(minT / DAY) === Math.floor(maxT / DAY)) {
      wrap.classList.add('hidden');
      graph.setTimeRange(null, null);
      return;
    }
    wrap.classList.remove('hidden');
    const span = maxT - minT;
    const minIn = document.getElementById('scrub-min');
    const maxIn = document.getElementById('scrub-max');
    const minLabel = document.getElementById('scrub-min-label');
    const maxLabel = document.getElementById('scrub-max-label');
    const fmt = (t) => new Date(t).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
    minIn.value = 0;
    maxIn.value = 1000;
    function apply() {
      let lo = +minIn.value, hi = +maxIn.value;
      if (lo > hi) [lo, hi] = [hi, lo];
      const t0 = minT + (lo / 1000) * span;
      const t1 = minT + (hi / 1000) * span;
      minLabel.textContent = fmt(t0);
      maxLabel.textContent = fmt(t1);
      graph.setTimeRange(t0, t1);
    }
    minIn.oninput = apply;
    maxIn.oninput = apply;
    apply();
  }

  await loadGraph('', null);
  setupScrubber();
  updateStatus();
  return () => graph.destroy();
});

// ── Context ────────────────────────────────────────────────────────────────

route('/context', async () => {
  const app = document.getElementById('app');
  let config;
  try { config = await api.config.get(); } catch (e) { config = { context: { default_budget: 8192, high_priority_reserve: 0.6, max_recent_turns: 12, max_engrams: 10 } }; }

  app.innerHTML = `
    <div class="page context-page">
      <h2>Context Assembly Config</h2>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Token Budget</div>
        <div class="config-row">
          <label>Default budget</label>
          <input type="range" id="cfg-budget" min="1024" max="32768" step="1024" value="${config.context?.default_budget || 8192}">
          <span id="cfg-budget-val" class="mono">${config.context?.default_budget || 8192}</span>
        </div>
        <div class="config-row">
          <label>High-priority reserve</label>
          <input type="range" id="cfg-reserve" min="10" max="100" step="5" value="${Math.round((config.context?.high_priority_reserve || 0.6) * 100)}">
          <span id="cfg-reserve-val" class="mono">${Math.round((config.context?.high_priority_reserve || 0.6) * 100)}%</span>
        </div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Retrieval Config</div>
        <div class="config-row">
          <label>Max recent turns</label>
          <input type="number" id="cfg-turns" min="1" max="50" value="${config.context?.max_recent_turns || 12}" class="input-sm">
        </div>
        <div class="config-row">
          <label>Max engrams per assembly</label>
          <input type="number" id="cfg-max-engrams" min="1" max="50" value="${config.context?.max_engrams || 10}" class="input-sm">
        </div>
      </div>

      <div class="panel">
        <div class="panel-header">Live Preview</div>
        <div class="config-row">
          <input type="text" id="cfg-test-query" class="search-input" placeholder="Test query: What should I work on next?" style="flex:1;">
          <button id="cfg-assemble-btn" class="btn btn-primary">Assemble</button>
        </div>
        <div id="cfg-preview" class="preview-box"></div>
      </div>

      <button id="cfg-save" class="btn btn-primary" style="margin-top:1rem;">Save config</button>
    </div>
  `;

  document.getElementById('cfg-budget').oninput = function() {
    document.getElementById('cfg-budget-val').textContent = this.value;
  };
  document.getElementById('cfg-reserve').oninput = function() {
    document.getElementById('cfg-reserve-val').textContent = this.value + '%';
  };

  document.getElementById('cfg-assemble-btn').onclick = async () => {
    const q = document.getElementById('cfg-test-query').value || 'What should I work on next?';
    const budget = parseInt(document.getElementById('cfg-budget').value);
    const prev = document.getElementById('cfg-preview');
    prev.innerHTML = '<div class="loading-sm">Assembling…</div>';
    try {
      const r = await api.context.assemble({ query: q, token_budget: budget });
      prev.innerHTML = `
        <div class="faint" style="margin-bottom:0.5rem;">${r.token_count || 0} / ${budget} tokens · ${r.engrams_retrieved || 0} engrams · ${r.took_ms || 0}ms</div>
        <div class="messages-preview">${(r.messages || []).map(m =>
          `<div class="msg-row"><span class="msg-role">[${m.role}]</span> ${esc((m.content || '').slice(0, 300))}${(m.content || '').length > 300 ? '…' : ''}</div>`
        ).join('')}</div>`;
    } catch (e) {
      prev.innerHTML = `<div class="error">Assembly failed: ${esc(e.message)}</div>`;
    }
  };

  document.getElementById('cfg-save').onclick = async () => {
    await api.config.update({
      context: {
        default_budget: parseInt(document.getElementById('cfg-budget').value),
        high_priority_reserve: parseInt(document.getElementById('cfg-reserve').value) / 100,
        max_recent_turns: parseInt(document.getElementById('cfg-turns').value),
        max_engrams: parseInt(document.getElementById('cfg-max-engrams').value),
      }
    });
    toast('Config saved', 'ok');
  };

  updateStatus();
});

// ── Consolidation ──────────────────────────────────────────────────────────

route('/consolidation', async () => {
  const app = document.getElementById('app');
  let history, patterns, stats;
  try { history = await api.consolidate.history(); } catch (e) { history = []; }
  try { stats = await api.stats(); } catch (e) { stats = null; }
  try { patterns = await api.patterns({ query: '', min_engrams: 3 }); } catch (e) { patterns = null; }

  app.innerHTML = `
    <div class="page consolidation-page">
      <h2>Consolidation History</h2>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Run History</div>
        ${(Array.isArray(history) ? history : []).length ? history.map(r => `
          <div class="feed-item">
            <span class="faint">${r.run_at || '?'}</span>
            <span class="badge badge-semantic">${r.type || '?'}</span>
            <span>${r.episodes_processed || 0} episodes</span>
            <span>${r.semantics_created || 0} promoted</span>
            <span>${r.engrams_decayed || 0} decayed</span>
            ${r.notes ? `<span class="faint">${esc(r.notes)}</span>` : ''}
          </div>
        `).join('') : '<div class="faint" style="padding:1rem;">No consolidation runs yet.</div>'}
      </div>

      <div class="panel-grid">
        <div class="panel">
          <div class="panel-header">Stats</div>
          <div class="health-list">
            <div class="health-row">Last consolidation: <span>${stats?.last_consolidation || 'never'}</span></div>
            <div class="health-row">Last decay: <span>${stats?.last_decay || 'never'}</span></div>
            <div class="health-row">Total links: <span>${stats?.total_links || 0}</span></div>
            <div class="health-row">Embeddings: <span>${stats?.total_embeddings || 0}</span></div>
          </div>
        </div>
        <div class="panel">
          <div class="panel-header">Patterns</div>
          ${patterns && patterns.pattern ? `
            <div class="pattern-card">
              <div class="card-content">${esc(patterns.pattern.description || 'No patterns detected.')}</div>
              <div class="faint">sample: ${patterns.pattern.sample_size || 0} engrams</div>
            </div>
          ` : '<div class="faint" style="padding:1rem;">No temporal patterns detected yet.</div>'}
        </div>
      </div>

      <div style="margin-top:1rem;display:flex;gap:0.5rem;">
        <button class="btn" id="btn-decay">Run decay now</button>
        <button class="btn" id="btn-consolidate">Run consolidation now</button>
      </div>
    </div>
  `;

  document.getElementById('btn-decay').onclick = async () => {
    const r = await api.consolidate.decay();
    toast(`Decayed: ${r.decayed || 0}, Strengthened: ${r.strengthened || 0}, Pruned: ${r.pruned || 0}`, 'ok');
    render();
  };
  document.getElementById('btn-consolidate').onclick = async () => {
    const r = await api.consolidate.weekly();
    toast(`Promoted: ${r.promoted_to_semantic || 0}, Pruned imagined: ${r.pruned_imagined || 0}`, 'ok');
    render();
  };

  updateStatus();
});

// ── WebAuthn (passkey) plumbing ────────────────────────────────────────────
// Fresh implementation for the relay's ceremonies. The relay speaks
// base64url strings (webauthn-rs JSON format); the browser API needs
// ArrayBuffers — convert both ways. No `hints` are ever sent (browser
// defaults), origin is always window.location.origin, and SecurityError
// surfaces as an RP ID/origin mismatch.

function b64urlFromBuf(buf) {
  const bytes = new Uint8Array(buf);
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function bufFromB64url(s) {
  const b64 = String(s).replace(/-/g, '+').replace(/_/g, '/');
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes.buffer;
}

function encodeCredential(c) {
  const r = c.response;
  return {
    id: c.id,
    rawId: b64urlFromBuf(c.rawId),
    type: c.type,
    response: {
      clientDataJSON: b64urlFromBuf(r.clientDataJSON),
      attestationObject: r.attestationObject ? b64urlFromBuf(r.attestationObject) : undefined,
      authenticatorData: r.authenticatorData ? b64urlFromBuf(r.authenticatorData) : undefined,
      signature: r.signature ? b64urlFromBuf(r.signature) : undefined,
      userHandle: r.userHandle ? b64urlFromBuf(r.userHandle) : null,
      transports: r.getTransports ? r.getTransports() : undefined,
    },
    authenticatorAttachment: c.authenticatorAttachment || undefined,
    clientExtensionResults: c.clientExtensionResults || {},
  };
}

async function webauthnRegister(server) {
  try {
    const start = await api.account.registerStart(server, window.location.origin);
    const pub = start.challenge.publicKey;
    pub.challenge = bufFromB64url(pub.challenge);
    pub.user.id = bufFromB64url(pub.user.id);
    for (const ec of (pub.excludeCredentials || [])) ec.id = bufFromB64url(ec.id);
    const credential = await navigator.credentials.create({ publicKey: pub });
    const res = await api.account.registerFinish(
      server, window.location.origin, start.challenge_id, encodeCredential(credential));
    api.account.setToken(res.session_token);
    // Registration now only ATTACHES a passkey to a signed-in account —
    // account creation is email+password on the login screen, no wizard.
    toast(res.already_registered ? 'Passkey already registered — signed in' : 'Passkey added', 'ok');
    render();
  } catch (e) {
    if (e.name === 'SecurityError') {
      toast('Passkey blocked: RP ID/origin mismatch — the sync server must allow this origin (--rp-id / --origin, see SYNC.md)', 'error');
    } else if (e.name === 'NotAllowedError') {
      toast('Passkey registration cancelled or not allowed by the browser', 'error');
    } else {
      toast(e.message || 'Passkey registration failed', 'error');
    }
  }
}

async function webauthnLogin(server) {
  try {
    const start = await api.account.loginStart(server, window.location.origin);
    const pub = start.challenge.publicKey;
    pub.challenge = bufFromB64url(pub.challenge);
    for (const ac of (pub.allowCredentials || [])) ac.id = bufFromB64url(ac.id);
    const credential = await navigator.credentials.get({ publicKey: pub });
    const res = await api.account.loginFinish(
      server, window.location.origin, start.challenge_id, encodeCredential(credential));
    api.account.setToken(res.session_token);
    toast('Passkey verified — signed in', 'ok');
    render();
  } catch (e) {
    if (e.code === 'no_passkeys') {
      toast('No passkeys registered on this server yet — register one first', 'error');
    } else if (e.name === 'SecurityError') {
      toast('Passkey blocked: RP ID/origin mismatch — the sync server must allow this origin (--rp-id / --origin, see SYNC.md)', 'error');
    } else if (e.name === 'NotAllowedError') {
      toast('Sign-in cancelled or not allowed', 'error');
    } else {
      toast(e.message || 'Sign-in failed', 'error');
    }
  }
}

// Shared pairing-code panel: used by the login screen's Add-device wizard
// and the Settings → Account & Sync panel. The code is single-use and
// expires in 10 minutes (server-side); copy buttons wire via delegation-
// style data attributes because the host containers differ.
function pairCodeHtml(code) {
  const base = `engram pair ${code}`;
  return `
    <div class="settings-note">
      <strong>Pair this machine within 10 minutes:</strong><br>
      <label class="faint" for="pair-name">Device name (optional — shown in the roster):</label>
      <input id="pair-name" type="text" maxlength="128"
        placeholder="e.g. MacBook Air, home box" style="width:100%;margin:0.25rem 0 0.75rem;">
      <div class="pair-code mono">${esc(code)}</div>
      <div class="pair-command mono" id="pair-cmd">${esc(base)}</div>
      <div class="faint">Run the command on the machine. The device appears in Account &amp; Sync → Devices after its first sync. Codes are single-use; this one expires in 10 minutes.</div>
    </div>
    <div class="mutation" style="display:flex;gap:0.5rem;padding:0 1rem 1rem;flex-wrap:wrap;">
      <button class="btn btn-sm" data-copy="${esc(code)}">Copy code</button>
      <button class="btn btn-sm" data-copy-cmd="${esc(base)}">Copy command</button>
    </div>`;
}

// The copy-command button reads the name input at click time so the copied
// command always matches what the user sees. Names are trimmed, stripped of
// shell-significant characters, and capped at the relay's 128-char limit.
function pairCommand(base, nameInput) {
  const nm = (nameInput?.value || '').trim().replace(/["\\`$]/g, '').slice(0, 128);
  return nm ? `${base} --name "${nm}"` : base;
}

function wirePairCodeCopies(el) {
  const nameInput = el.querySelector('#pair-name');
  const cmdEl = el.querySelector('#pair-cmd');
  const base = el.querySelector('[data-copy-cmd]')?.getAttribute('data-copy-cmd') || '';
  nameInput?.addEventListener('input', () => { if (cmdEl) cmdEl.textContent = pairCommand(base, nameInput); });
  el.querySelectorAll('[data-copy]').forEach(b => {
    b.onclick = async () => {
      try { await navigator.clipboard.writeText(b.getAttribute('data-copy')); toast('Copied', 'ok'); }
      catch { toast('Copy failed — copy the text manually', 'error'); }
    };
  });
  el.querySelectorAll('[data-copy-cmd]').forEach(b => {
    b.onclick = async () => {
      try { await navigator.clipboard.writeText(pairCommand(base, nameInput)); toast('Copied', 'ok'); }
      catch { toast('Copy failed — copy the text manually', 'error'); }
    };
  });
}

// ── Settings ───────────────────────────────────────────────────────────────

route('/settings', async () => {
  const app = document.getElementById('app');
  let config, audit, team;
  try { config = await api.config.get(); } catch (e) { config = { vault_path: '~/.engram/vaults/default', encryption: 'sqlcipher' }; }
  try { audit = await api.privacy.audit(); } catch (e) { audit = null; }
  try { team = await api.teams.status(); } catch (e) { team = null; }

  const ctx = config.context || {};
  const sched = config.schedule || {};
  const emb = config.embedding || {};
  const sync = config.sync || {};
  const breakdown = audit?.breakdown || {};

  const breakdownRows = (items, key, iconFn) => (items || []).length
    ? items.map(i => `<div class="health-row">${iconFn ? iconFn(i[key]) + ' ' : ''}${esc(i[key] || '—')}<span class="ml-auto mono">${i.count}</span></div>`).join('')
    : '<div class="health-row faint">No data.</div>';

  app.innerHTML = `
    <div class="page settings-page">
      <h2>Vault Settings</h2>

      <div class="settings-tabs">
        <button class="tab-btn" data-tab="account-sync">Account &amp; Sync</button>
        <button class="tab-btn" data-tab="vault">Vault</button>
        <button class="tab-btn" data-tab="privacy">Privacy</button>
        <button class="tab-btn" data-tab="automation">Automation</button>
      </div>

      <div class="tab-panel" id="tab-vault">
      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Vault</div>
        <div class="health-list">
          <div class="health-row">Name: <span class="mono">default</span></div>
          <div class="health-row">Path: <span class="mono">${esc(config.vault_path || '~/.engram/vaults/default')}</span></div>
          <div class="health-row">Encryption: <span class="ok">${esc(config.encryption || 'sqlcipher')}</span></div>
          <div class="health-row">Version: <span class="mono">${esc(config.version || '?')}</span></div>
        </div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Context Defaults</div>
        <div class="config-row">
          <label>Default token budget</label>
          <input type="range" id="set-budget" min="1024" max="32768" step="1024" value="${ctx.default_budget || 8192}">
          <span id="set-budget-val" class="mono">${ctx.default_budget || 8192}</span>
        </div>
        <div class="config-row">
          <label>High-priority reserve</label>
          <input type="range" id="set-reserve" min="10" max="100" step="5" value="${Math.round((ctx.high_priority_reserve || 0.6) * 100)}">
          <span id="set-reserve-val" class="mono">${Math.round((ctx.high_priority_reserve || 0.6) * 100)}%</span>
        </div>
        <div class="config-row">
          <label>Max engrams per assembly</label>
          <input type="number" id="set-max-engrams" min="1" max="50" value="${ctx.max_engrams || 10}" class="input-sm">
        </div>
        <div class="config-row">
          <label>Max recent turns</label>
          <input type="number" id="set-max-turns" min="1" max="50" value="${ctx.max_recent_turns || 12}" class="input-sm">
        </div>
        <div class="mutation" style="padding:0 1rem 1rem;"><button class="btn btn-primary" id="set-context-save">Save context defaults</button></div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Schedule</div>
        <div class="config-row">
          <label>Decay every (hours)</label>
          <input type="number" id="set-decay-h" min="1" max="168" value="${sched.decay_interval_hours || 1}" class="input-sm">
          <label class="checkbox-label"><input type="checkbox" id="set-auto-decay" ${sched.auto_decay !== false ? 'checked' : ''}> auto</label>
        </div>
        <div class="config-row">
          <label>Consolidation every (hours)</label>
          <input type="number" id="set-cons-h" min="1" max="720" value="${sched.consolidation_interval_hours || 24}" class="input-sm">
          <label class="checkbox-label"><input type="checkbox" id="set-auto-cons" ${sched.auto_consolidation !== false ? 'checked' : ''}> auto</label>
        </div>
        <div class="mutation" style="padding:0 1rem 1rem;"><button class="btn btn-primary" id="set-schedule-save">Save schedule</button></div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Embeddings</div>
        <div class="health-list">
          <div class="health-row">Model: <span class="mono">${esc(emb.model || '—')}</span></div>
          <div class="health-row">Dimensions: <span class="mono">${emb.dimensions || '—'}</span></div>
          <div class="health-row">Status: <span class="${emb.enabled ? 'ok' : 'faint'}">${emb.enabled ? 'enabled' : 'disabled'}</span></div>
        </div>
      </div>

      </div>

      <div class="tab-panel active" id="tab-account-sync">
      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Remote Access</div>
        <div class="settings-note">
          API key authentication is configured server-side via the <code>ENGRAMD_API_KEY</code>
          environment variable when <code>engramd</code> starts. It cannot be changed from the UI.
          See <code>docs/engram-product/DEPLOY.md</code> for exposing the vault behind Caddy.
        </div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Account (Sync Server)</div>
        <div class="settings-note">
          Accounts are standalone passkeys — no email, no name. The account lives on the
          sync server (<code>${esc(sync.server_url || 'not set')}</code>) and owns the API keys
          this vault syncs with. Sessions are Bearer tokens stored in this browser only.
        </div>
        <div id="account-body" class="health-list" style="padding:0 1rem 1rem;"><div class="loading-sm">Loading…</div></div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Sync &amp; Team</div>
        <div class="settings-note">
          Shared-vault sync: every team member runs <code>engramd</code> with the same vault
          passphrase and <code>vault_id</code> against one sync server. Content stays
          end-to-end encrypted — the server only sees device IDs and blob counts.
          Sync settings are read at daemon startup: <strong>restart engramd after saving</strong>,
          and the daemon must be started with the vault passphrase.
        </div>
        <div class="health-list">
          <div class="health-row">Vault ID: <span class="mono">${esc(team?.vault_id || 'not set')}</span>
            <button class="btn btn-sm" id="team-copy-vault" ${team?.vault_id ? '' : 'disabled'}>Copy</button></div>
          <div class="health-row">Server: <span id="team-reach">—</span></div>
        </div>
        <div class="config-row">
          <label>Team name</label>
          <input type="text" id="team-name" class="input-sm" value="${esc(sync.name || '')}" placeholder="e.g. core-team">
        </div>
        <div class="config-row">
          <label>Sync enabled</label>
          <label class="checkbox-label"><input type="checkbox" id="team-sync-enabled" ${sync.enabled ? 'checked' : ''}> push &amp; pull on interval</label>
        </div>
        <div class="config-row">
          <label>Server URL</label>
          <input type="text" id="team-server-url" class="input-sm" value="${esc(sync.server_url || '')}" placeholder="https://sync.example.com" style="min-width:220px;">
        </div>
        <div class="config-row">
          <label>Interval (seconds)</label>
          <input type="number" id="team-interval" min="5" max="86400" value="${sync.interval_secs || 60}" class="input-sm">
        </div>
        <div class="mutation" style="padding:0 1rem 1rem;display:flex;gap:0.5rem;flex-wrap:wrap;">
          <button class="btn btn-primary" id="team-save">Save sync settings</button>
          <button class="btn" id="team-sync-now" ${sync.enabled ? '' : 'disabled'}>Sync now</button>
        </div>
        <div id="team-devices" class="health-list"></div>
        <div class="settings-note">
          <strong>Honest caveats (shared-vault v0):</strong>
          <ul style="margin:0.25rem 0 0 1.25rem;padding:0;">
            <li>Team membership is just a shared passphrase — no per-member revocation or audit.</li>
            <li>Anyone holding the sync server's API key can read every vault on that server.</li>
            <li>Machine-keyed vaults (no passphrase) cannot sync.</li>
            <li>Last-writer-wins: an edit from a device with a slower clock can be overwritten.</li>
            <li>The device list counts pushes only — a teammate appears after their first push.</li>
          </ul>
        </div>
      </div>

      </div>

      <div class="tab-panel" id="tab-privacy">
      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Privacy — What's Stored</div>
        ${audit ? `
          <div class="health-list">
            <div class="health-row">Total memories: <span class="mono">${audit.total_memories ?? 0}</span></div>
            <div class="health-row">Oldest: <span>${audit.oldest_date ? esc(String(audit.oldest_date).slice(0, 10)) : '—'}</span> · Newest: <span>${audit.newest_date ? esc(String(audit.newest_date).slice(0, 10)) : '—'}</span></div>
            <div class="health-row">Avg age: <span>${(audit.avg_age_days ?? 0).toFixed ? audit.avg_age_days.toFixed(1) : (audit.avg_age_days ?? 0)} days</span></div>
            <div class="health-row">Size: <span class="mono">${esc(audit.estimated_db_size_human || formatBytes(audit.db_size_bytes || 0))}</span></div>
            <div class="health-row"><span class="ok">●</span> Local-only vault — nothing leaves this machine unless sync is configured.</div>
          </div>
          <div class="audit-grid">
            <div>
              <div class="panel-header" style="padding:0.5rem 1rem 0.25rem;">By layer</div>
              <div class="health-list">${breakdownRows(breakdown.by_layer, 'layer', (l) => layerIcon(l))}</div>
            </div>
            <div>
              <div class="panel-header" style="padding:0.5rem 1rem 0.25rem;">By source</div>
              <div class="health-list">${breakdownRows(breakdown.by_source, 'source', (s) => sourceIcon(s))}</div>
            </div>
            <div>
              <div class="panel-header" style="padding:0.5rem 1rem 0.25rem;">By project</div>
              <div class="health-list">${breakdownRows(breakdown.by_project, 'project', null)}</div>
            </div>
          </div>
        ` : '<div class="settings-note">Privacy audit unavailable (requires engramd ≥ privacy routes).</div>'}
      </div>

      <div class="panel danger-zone" style="margin-bottom:1rem;">
        <div class="panel-header">Danger Zone — Purge Memories</div>
        <div class="settings-note">
          Permanently delete memories matching <em>at least one</em> criterion. This cannot be undone.
          Consider <button class="link-btn" id="purge-export-first">exporting first</button>.
        </div>
        <div class="purge-form">
          <select id="purge-source" class="filter-select">
            <option value="">Any source</option>
            ${(breakdown.by_source || []).map(s => `<option value="${esc(s.source)}">${esc(s.source)}</option>`).join('')}
          </select>
          <select id="purge-layer" class="filter-select">
            <option value="">Any layer</option>
            <option value="episodic">Episodic</option>
            <option value="semantic">Semantic</option>
            <option value="imagined">Imagined</option>
          </select>
          <input type="text" id="purge-project" class="input-sm" placeholder="Project (optional)" style="min-width:140px;">
          <input type="date" id="purge-before" class="input-sm" title="Delete memories created before this date">
          <button class="btn btn-danger" id="purge-btn">Purge…</button>
        </div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Import / Export</div>
        <div class="mutation" style="display:flex;gap:0.5rem;padding:1rem;flex-wrap:wrap;">
          <button class="btn" id="btn-export">Export (JSONL)</button>
          <label class="btn" style="cursor:pointer;">
            Import
            <input type="file" id="import-file" accept=".jsonl,.json" style="display:none;">
          </label>
          <button class="btn" id="import-btn" style="display:none;">Upload & Import</button>
        </div>
      </div>

      </div>

      <div class="tab-panel" id="tab-automation">
      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Saved Searches <button class="btn btn-sm" id="ss-check-all">Check all now</button></div>
        <div class="ss-form">
          <input type="text" id="ss-query" class="search-input" placeholder="Watch query… e.g. debugging">
          <select id="ss-layer" class="filter-select">
            <option value="">Any layer</option>
            <option value="episodic">Episodic</option>
            <option value="semantic">Semantic</option>
            <option value="imagined">Imagined</option>
          </select>
          <label class="ss-notify"><input type="checkbox" id="ss-notify"> Notify on new matches</label>
          <button id="ss-add" class="btn btn-primary btn-sm">Save</button>
        </div>
        <div id="ss-list" class="ss-list"><div class="loading-sm">Loading…</div></div>
      </div>

      <div class="panel">
        <div class="panel-header">Onboarding &amp; Demo Data</div>
        <div class="settings-note">
          Demo memories are everyday examples tagged with <code>demo</code> (project
          <code>demo</code>) so the Explorer and Graph come alive. Remove them all at any
          time — only memories tagged <code>demo</code> are touched.
        </div>
        <div style="padding:0 1rem 1rem;display:flex;gap:0.5rem;flex-wrap:wrap;">
          <a class="btn" href="#/tour">Open Tour &amp; Demo</a>
          <button class="btn" id="btn-load-demo">Load demo memories</button>
          <button class="btn" id="btn-remove-demo">Remove demo memories</button>
          <button class="btn" id="btn-replay-onboarding">Replay onboarding wizard</button>
        </div>
      </div>
      </div>
    </div>
  `;

  // ── Settings tabs: one group of panels at a time, choice persisted
  (function initSettingsTabs() {
    const tabs = document.querySelectorAll('.settings-tabs .tab-btn');
    const panels = document.querySelectorAll('.tab-panel');
    const saved = localStorage.getItem('engram-settings-tab');
    const activate = (btn) => {
      tabs.forEach(t => t.classList.toggle('active', t === btn));
      panels.forEach(p => p.classList.toggle('active', p.id === 'tab-' + btn.dataset.tab));
      localStorage.setItem('engram-settings-tab', btn.dataset.tab);
    };
    const initial = [...tabs].find(t => t.dataset.tab === saved) || tabs[0];
    activate(initial);
    tabs.forEach(btn => { btn.onclick = () => activate(btn); });
  })();

  // ── Slider value labels
  document.getElementById('set-budget').oninput = function() {
    document.getElementById('set-budget-val').textContent = this.value;
  };
  document.getElementById('set-reserve').oninput = function() {
    document.getElementById('set-reserve-val').textContent = this.value + '%';
  };

  // ── Sync & team: roster, reachability, save, copy
  (function renderTeam() {
    const devicesEl = document.getElementById('team-devices');
    const reachEl = document.getElementById('team-reach');
    if (!team) {
      devicesEl.innerHTML = `<div class="health-row faint">${sync.enabled ? 'Team status unavailable.' : 'Sync not enabled — configure it below and restart engramd.'}</div>`;
      if (reachEl) reachEl.textContent = sync.enabled ? 'unknown' : 'sync disabled';
      return;
    }
    if (reachEl) {
      reachEl.innerHTML = team.remote_reachable
        ? '<span class="ok">● reachable</span>'
        : '<span class="error">● unreachable</span>';
    }
    const rows = (team.devices || []).map(d => `
      <div class="health-row">${d.label ? `<span>${esc(d.label)}</span> <span class="mono faint">${esc(d.device_id || '?')}</span>` : `<span class="mono">${esc(d.device_id || '?')}</span>`}${d.is_self ? ' <span class="badge badge-semantic">this device</span>' : ''}<span class="ml-auto faint">${d.blob_count || 0} blobs · ${esc(String(d.last_seen || '—').slice(0, 19))}</span></div>`).join('');
    devicesEl.innerHTML = rows || '<div class="health-row faint">No devices have pushed to this vault yet.</div>';
    const lp = team.last_push ? String(team.last_push).slice(0, 19) : '—';
    const pl = team.last_pull ? String(team.last_pull).slice(0, 19) : '—';
    devicesEl.insertAdjacentHTML('beforeend', `<div class="health-row faint">Last push: <span class="mono">${lp}</span> · Last pull: <span class="mono">${pl}</span></div>`);
    if (team.last_push_error) {
      devicesEl.insertAdjacentHTML('beforeend', `<div class="health-row error">Last push error: ${esc(team.last_push_error)}</div>`);
    }
  })();

  // ── Account panel: passkey sign-in/register, quota bars, API keys
  (async function renderAccount() {
    const bodyEl = document.getElementById('account-body');
    const server = (sync.server_url || '').trim();
    if (!server) {
      bodyEl.innerHTML = '<div class="health-row faint">Set a sync server URL (Sync &amp; Team panel) to use accounts.</div>';
      return;
    }
    const renderSignedOut = () => {
      bodyEl.innerHTML = `
        <div class="health-row faint">Not signed in — no account session on this browser.</div>
        <div class="mutation" style="display:flex;gap:0.5rem;padding:0 1rem 1rem;">
          <button class="btn btn-primary" id="acct-signin">Sign in</button>
        </div>`;
      document.getElementById('acct-signin').onclick = () => navigate('#/login');
    };
    if (!api.account.token()) { renderSignedOut(); return; }

    let acct;
    try { acct = await api.account.get(server); }
    catch (e) {
      if (e.status === 401) { api.account.setToken(null); renderSignedOut(); return; }
      bodyEl.innerHTML = `<div class="health-row error">Account unreachable: ${esc(e.message)}</div>`;
      return;
    }

    const q = acct.quota || {};
    const quotaBar = (label, used, limit, fmt) => {
      if (!limit) return `<div class="health-row">${label}: <span class="mono">${fmt ? fmt(used) : used}</span> <span class="faint">(unlimited)</span></div>`;
      const pct = Math.min(100, Math.round((used / limit) * 100));
      const color = pct > 90 ? 'var(--decaying)' : pct > 60 ? 'var(--episodic)' : 'var(--grounded)';
      return `<div class="health-row">${label}: <span class="mono">${fmt ? fmt(used) : used} / ${fmt ? fmt(limit) : limit}</span>
        <div class="mini-bar"><div class="mini-bar-fill" style="width:${pct}%;background:${color};"></div></div></div>`;
    };
    const activeKeys = (acct.keys || []).filter(k => !k.revoked);
    const revokedKeys = (acct.keys || []).filter(k => k.revoked);
    bodyEl.innerHTML = `
      <div class="health-row">Signed in as <span class="mono">${esc((acct.account_id || '').slice(0, 13))}…</span></div>
      <div class="mutation" style="display:flex;gap:0.5rem;padding:0 1rem 0.5rem;flex-wrap:wrap;">
        <button class="btn btn-sm btn-danger" id="acct-logout">Sign out</button>
      </div>
      ${quotaBar('Devices', q.devices_used || 0, q.devices || 0)}
      ${quotaBar('Bytes', q.bytes_used || 0, q.bytes || 0, formatBytes)}
      <div class="health-row" style="margin-top:0.5rem;"><strong>API keys</strong>
        <span class="ml-auto"><button class="btn btn-sm" id="acct-pair">Pair a device</button> <button class="btn btn-sm btn-primary" id="acct-new-key">New key${(team?.vault_id || sync.vault_id) ? ' (this vault)' : ''}</button></span></div>
      <div id="acct-pair-once"></div>
      ${activeKeys.map(k => `
        <div class="health-row"><span class="mono">${esc(k.key_prefix)}…</span>
          <span class="faint">${k.vault_id ? 'scoped to ' + esc(k.vault_id) : 'all vaults'} · ${esc(String(k.created_at || '').slice(0, 10))}</span>
          <span class="ml-auto"><button class="btn btn-sm btn-danger" data-revoke="${esc(k.id)}">Revoke</button></span></div>`).join('')
        || '<div class="health-row faint">No keys yet — create one to let this device sync.</div>'}
      ${revokedKeys.length ? `<div class="health-row faint">${revokedKeys.length} revoked key${revokedKeys.length > 1 ? 's' : ''} (history)</div>` : ''}
      <div id="acct-key-once"></div>`;

    // ── Email+password credentials, passkeys, recovery ────────────────────
    const acctId = acct.account_id;
    let creds = null;
    try { creds = await api.account.credentials(server); } catch {}
    const acctExtra = document.createElement('div');
    acctExtra.innerHTML = creds
      ? `
        <div class="health-row" style="margin-top:0.5rem;"><strong>Account</strong></div>
        <div class="health-row"><span class="faint">Email</span> <span class="mono">${esc(creds.email || '—')}</span></div>
        <div class="health-row"><span class="faint">Recovery phrase</span>
          <span class="ml-auto">${creds.has_recovery_key
            ? `<span class="ok">● set ${creds.recovery_created_at ? '· ' + esc(String(creds.recovery_created_at).slice(0, 10)) : ''}</span>`
            : '<span class="error">● NOT set — set it now</span>'}
          <button class="btn btn-sm" id="acct-rotrec">${creds.has_recovery_key ? 'Rotate' : 'Set up'}</button></span></div>
        ${creds.has_password ? `
        <div class="health-row"><span class="faint">Password</span>
          <span class="ml-auto"><button class="btn btn-sm" id="acct-chpw">Change password</button></span></div>
        <div id="acct-chpw-form" style="display:none;"></div>` : `
        <div class="health-row faint">Passkey-only account — recreate it from the login screen to add a password.</div>`}
        <div class="health-row"><strong>Passkeys</strong>
          <span class="ml-auto"><button class="btn btn-sm btn-primary" id="acct-add-passkey">Add passkey</button></span></div>
        ${(creds.passkeys || []).map(p => `
          <div class="health-row"><span class="mono">${esc(String(p.credential_id || '').slice(0, 16))}…</span>
            <span class="faint">${esc(String(p.created_at || '').slice(0, 10))}</span>
            <span class="ml-auto"><button class="btn btn-sm btn-danger" data-detach="${esc(p.credential_id)}">Detach</button></span></div>`).join('')
          || '<div class="health-row faint">None — add one for faster sign-in.</div>'}
        <div class="health-row faint">Passkeys are optional extras; your password is the account. The recovery phrase is the only way back into your vaults if the password is lost.</div>`
      : '<div class="health-row faint">No credentials — passkey-only account.</div>';
    bodyEl.appendChild(acctExtra);

    acctExtra.querySelector('#acct-add-passkey').onclick = () => webauthnRegister(server);

    acctExtra.querySelectorAll('[data-detach]').forEach(btn => {
      btn.onclick = async () => {
        const cid = btn.getAttribute('data-detach');
        let password = null;
        if (creds && creds.has_password) {
          password = prompt('Detaching a passkey needs your account password (server verifies it fresh):');
          if (!password) { toast('Password required to detach', 'error'); return; }
        }
        if (!confirm('Detach this passkey? Signing in with it stops working.')) return;
        try {
          await api.account.detachPasskey(server, cid, password);
          toast('Passkey detached', 'ok');
          renderAccount();
        } catch (e) {
          if (e.code === 'invalid_password') toast('Incorrect account password', 'error');
          else if (e.code === 'password_required') toast('Enter your account password to detach', 'error');
          else toast(e.message, 'error');
        }
      };
    });

    // Account key A for rewrap/rotate: in memory after password signin; if
    // this tab doesn't have it, ask for the account password once.
    const ensureA = async (reason) => {
      let A = unlock.getAccountKey(acctId);
      if (A) return A;
      const pw = prompt(reason);
      if (!pw) throw new Error('Account password required.');
      try {
        const blob = await api.account.getPasswordWrap(server);
        const wk = await unlock.deriveWithSalt(pw, unlock.b64decode(blob.salt_pw));
        try { A = await unlock.unwrapKey(wk, unlock.b64decode(blob.wrapped_a)); }
        finally { wk.fill(0); }
      } catch {
        throw new Error('Incorrect account password.');
      }
      unlock.setAccountKey(acctId, A);
      return A;
    };

    const rotBtn = acctExtra.querySelector('#acct-rotrec');
    if (rotBtn) rotBtn.onclick = async () => {
      const rnd = new Uint32Array(12);
      crypto.getRandomValues(rnd);
      const words = [];
      for (let i = 0; i < 12; i++) words.push(BIP39_WORDS[rnd[i] % 2048]);
      const phrase = words.join(' ');
      const ok = prompt('New recovery phrase — WRITE IT DOWN NOW. It replaces the old one and is shown only once.\n\n' + phrase + '\n\nType the first word to confirm:', '');
      if (ok === null) return;
      if (ok.trim().toLowerCase() !== words[0]) { toast('Confirmation word does not match — phrase not changed', 'error'); return; }
      try {
        const A = await ensureA('Rotating the recovery phrase needs your account password (or sign in with it first):');
        const saltRec = crypto.getRandomValues(new Uint8Array(16));
        const wk = await unlock.deriveWithSalt(phrase, saltRec);
        const wrapped = await unlock.wrapKey(wk, A);
        wk.fill(0);
        await api.account.putRecoveryWrap(server, unlock.b64encode(wrapped), unlock.b64encode(saltRec));
        toast('Recovery phrase rotated', 'ok');
        renderAccount();
      } catch (e) { toast(e.message, 'error'); }
    };

    const chpwBtn = acctExtra.querySelector('#acct-chpw');
    if (chpwBtn) chpwBtn.onclick = () => {
      const form = acctExtra.querySelector('#acct-chpw-form');
      const open = form.style.display !== 'block';
      form.style.display = open ? 'block' : 'none';
      if (open && !form.dataset.wired) {
        form.dataset.wired = '1';
        form.innerHTML = `
          <input id="chpw-cur" type="password" placeholder="Current password" autocomplete="current-password">
          <input id="chpw-new" type="password" placeholder="New password (12+ characters)" autocomplete="new-password">
          <input id="chpw-conf" type="password" placeholder="Confirm new password" autocomplete="new-password">
          <div class="mutation" style="padding:0 1rem 0.5rem;">
            <button class="btn btn-primary btn-sm" id="chpw-go">Change password</button>
          </div>`;
        acctExtra.querySelector('#chpw-go').onclick = async () => {
          const cur = acctExtra.querySelector('#chpw-cur').value;
          const np = acctExtra.querySelector('#chpw-new').value;
          const conf = acctExtra.querySelector('#chpw-conf').value;
          if (np.length < 12) { toast('New password must be 12+ characters', 'error'); return; }
          if (np !== conf) { toast('New passwords do not match', 'error'); return; }
          const btn = acctExtra.querySelector('#chpw-go');
          btn.disabled = true;
          try {
            await api.account.changePassword(server, cur, np);
            // Rewrap A under the new password so signin keeps opening vaults.
            const A = await ensureA('Re-linking your vault keys needs your account password:');
            const saltPw = crypto.getRandomValues(new Uint8Array(16));
            const wk = await unlock.deriveWithSalt(np, saltPw);
            try {
              const wrapped = await unlock.wrapKey(wk, A);
              await api.account.putPasswordWrap(server, unlock.b64encode(wrapped), unlock.b64encode(saltPw));
            } finally { wk.fill(0); }
            A.fill(0);
            toast('Password changed — vault keys re-linked', 'ok');
            renderAccount();
          } catch (e) {
            btn.disabled = false;
            if (e.code === 'invalid_password') toast('Current password is incorrect', 'error');
            else toast(e.message, 'error');
          }
        };
      }
    };

    document.getElementById('acct-logout').onclick = async () => {
      try { await api.account.logout(server); } catch {}
      api.account.setToken(null);
      // Also drop the vault's HTTP basic-auth gate: /auth/ui-logout answers
      // 401 with a fresh challenge (Caddy config), which makes the browser
      // clear its cached vault credentials. 401 is the EXPECTED reply, so
      // this fetch resolves. Then clear sessionStorage and return to the
      // branded login screen (no page reload).
      try { await fetch('/auth/ui-logout', { method: 'POST' }); } catch {}
      clearCreds();
      toast('Signed out', 'ok');
      navigate('#/login');
    };

    document.getElementById('acct-new-key').onclick = async () => {
      try {
        const vaultId = team?.vault_id || sync.vault_id || null;
        const k = await api.account.createKey(server, vaultId);
        document.getElementById('acct-key-once').innerHTML = `
          <div class="settings-note">
            <strong>Copy this API key now — it is shown only once.</strong> The server stores a hash.<br>
            <code class="mono" style="word-break:break-all;">${esc(k.api_key)}</code>
          </div>
          <div class="mutation" style="display:flex;gap:0.5rem;padding:0 1rem 1rem;flex-wrap:wrap;">
            <button class="btn btn-sm" id="acct-key-copy">Copy key</button>
            <button class="btn btn-sm btn-primary" id="acct-key-connect">Connect this device</button>
          </div>`;
        document.getElementById('acct-key-copy').onclick = async () => {
          try { await navigator.clipboard.writeText(k.api_key); toast('API key copied', 'ok'); }
          catch { toast('Copy failed — select the key text manually', 'error'); }
        };
        document.getElementById('acct-key-connect').onclick = async () => {
          try {
            const r = await api.config.update({ sync: { api_key: k.api_key } });
            if (!r.ok) { const err = await r.json().catch(() => ({})); toast(err.error?.message || 'Save failed', 'error'); return; }
            toast('API key saved to this vault — restart engramd to use it', 'ok');
          } catch (e) { toast(e.message || 'Save failed', 'error'); }
        };
      } catch (e) { toast(e.message || 'Key creation failed', 'error'); }
    };

    document.getElementById('acct-pair').onclick = async () => {
      const el = document.getElementById('acct-pair-once');
      el.innerHTML = '<div class="faint">Requesting a pairing code…</div>';
      try {
        const res = await api.account.pairCodes(server);
        el.innerHTML = pairCodeHtml(res.code);
        wirePairCodeCopies(el);
      } catch (e) {
        el.innerHTML = `<div class="error-panel"><p>${esc(e.message)}</p></div>`;
      }
    };

    bodyEl.querySelectorAll('[data-revoke]').forEach(btn => {
      btn.onclick = async () => {
        const keyId = btn.getAttribute('data-revoke');
        if (!confirm('Revoke this API key? Devices using it will stop syncing immediately.')) return;
        try {
          await api.account.revokeKey(server, keyId);
          toast('Key revoked', 'ok');
          render();
        } catch (e) { toast(e.message || 'Revoke failed', 'error'); }
      };
    });
  })();

  document.getElementById('team-copy-vault').onclick = async () => {
    const v = team?.vault_id;
    if (!v) return;
    try { await navigator.clipboard.writeText(v); toast('Vault ID copied', 'ok'); }
    catch (e) { toast('Copy failed', 'error'); }
  };

  // Field-wise merge on the server: send only what the panel edits — vault_id and
  // api_key are never overwritten from the UI (null fields are skipped).
  document.getElementById('team-save').onclick = async () => {
    try {
      const r = await api.config.update({
        sync: {
          enabled: document.getElementById('team-sync-enabled').checked,
          server_url: document.getElementById('team-server-url').value.trim() || null,
          interval_secs: parseInt(document.getElementById('team-interval').value) || 60,
          name: document.getElementById('team-name').value.trim() || null,
        }
      });
      if (!r.ok) { const err = await r.json().catch(() => ({})); toast(err.error?.message || err.error || 'Save failed', 'error'); return; }
      toast('Sync settings saved — restart engramd to apply', 'ok');
      render();
    } catch (e) { toast(e.message || 'Save failed', 'error'); }
  };

  document.getElementById('team-sync-now').onclick = async () => {
    try {
      await api.sync.now();
      toast('Sync triggered', 'ok');
    } catch (e) { toast(e.message || 'Sync failed', 'error'); }
  };

  // ── Save context defaults (server replaces the whole sub-object — send all fields)
  document.getElementById('set-context-save').onclick = async () => {
    try {
      const r = await api.config.update({
        context: {
          default_budget: parseInt(document.getElementById('set-budget').value),
          high_priority_reserve: parseInt(document.getElementById('set-reserve').value) / 100,
          max_engrams: parseInt(document.getElementById('set-max-engrams').value),
          max_recent_turns: parseInt(document.getElementById('set-max-turns').value),
        }
      });
      if (!r.ok) { const err = await r.json().catch(() => ({})); toast(err.error?.message || err.error || 'Save failed', 'error'); return; }
      toast('Context defaults saved', 'ok');
    } catch (e) { toast(e.message || 'Save failed', 'error'); }
  };

  // ── Save schedule
  document.getElementById('set-schedule-save').onclick = async () => {
    try {
      const r = await api.config.update({
        schedule: {
          decay_interval_hours: parseInt(document.getElementById('set-decay-h').value),
          consolidation_interval_hours: parseInt(document.getElementById('set-cons-h').value),
          auto_decay: document.getElementById('set-auto-decay').checked,
          auto_consolidation: document.getElementById('set-auto-cons').checked,
        }
      });
      if (!r.ok) { const err = await r.json().catch(() => ({})); toast(err.error?.message || err.error || 'Save failed', 'error'); return; }
      toast('Schedule saved', 'ok');
    } catch (e) { toast(e.message || 'Save failed', 'error'); }
  };

  // ── Export / import
  async function doExport() {
    const r = await api.export({ format: 'jsonl' });
    const memories = r.memories || [];
    const jsonl = memories.map(m => JSON.stringify(m)).join('\n');
    const blob = new Blob([jsonl], { type: 'application/x-ndjson' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `engrams-export-${new Date().toISOString().slice(0,10)}.jsonl`;
    a.click();
    return memories.length;
  }

  document.getElementById('btn-export').onclick = async () => {
    try {
      const n = await doExport();
      toast(`Exported ${n} memories`, 'ok');
    } catch (e) { toast(e.message || 'Export failed', 'error'); }
  };

  document.getElementById('purge-export-first').onclick = async () => {
    try {
      const n = await doExport();
      toast(`Exported ${n} memories — safe to purge`, 'ok');
    } catch (e) { toast(e.message || 'Export failed', 'error'); }
  };

  document.getElementById('import-btn').onclick = async () => {
    try {
      const file = document.getElementById('import-file').files[0];
      if (!file) return;
      const text = await file.text();
      // Parse JSONL or JSON, then send as { memories: [...] }
      let memories;
      try {
        const parsed = JSON.parse(text);
        memories = Array.isArray(parsed) ? parsed : (parsed.memories || [parsed]);
      } catch {
        memories = text.split('\n')
          .filter(line => line.trim())
          .map(line => { try { return JSON.parse(line); } catch {} return null; })
          .filter(Boolean);
      }
      const r = await api.import({ memories });
      toast(`Imported ${r.imported || 0} memories (${r.skipped || 0} skipped)`, 'ok');
    } catch (e) { toast(e.message || 'Import failed', 'error'); }
  };

  // ── Purge with confirmation modal
  document.getElementById('purge-btn').onclick = () => {
    const source = document.getElementById('purge-source').value;
    const layer = document.getElementById('purge-layer').value;
    const project = document.getElementById('purge-project').value.trim();
    const before = document.getElementById('purge-before').value;

    const criteria = {};
    const desc = [];
    if (source) { criteria.source = source; desc.push(`source = "${source}"`); }
    if (layer) { criteria.layer = layer; desc.push(`layer = "${layer}"`); }
    if (project) { criteria.project = project; desc.push(`project = "${project}"`); }
    if (before) { criteria.before_date = new Date(before + 'T00:00:00Z').toISOString(); desc.push(`created before ${before}`); }

    if (!desc.length) {
      toast('Select at least one purge criterion', 'error');
      return;
    }

    const root = document.getElementById('modal-root');
    root.innerHTML = `
      <div class="ob-overlay">
        <div class="ob-modal purge-modal">
          <h3>Confirm purge</h3>
          <p>Permanently delete all memories where <strong>${esc(desc.join(' AND '))}</strong>?</p>
          <p class="faint">This cannot be undone.</p>
          <div class="ob-actions">
            <button class="btn" id="purge-cancel">Cancel</button>
            <button class="btn btn-danger" id="purge-confirm">Delete permanently</button>
          </div>
        </div>
      </div>`;
    document.getElementById('purge-cancel').onclick = () => { root.innerHTML = ''; };
    document.getElementById('purge-confirm').onclick = async () => {
      try {
        const r = await api.privacy.purge(criteria);
        root.innerHTML = '';
        toast(`Purged ${r.deleted ?? r.purged ?? 0} memories`, 'warn');
        render();
      } catch (e) {
        root.innerHTML = '';
        toast(e.message || 'Purge failed', 'error');
      }
    };
  };

  // ── Onboarding & demo data
  document.getElementById('btn-load-demo').onclick = async () => {
    try {
      const r = await loadDemoMemories();
      if (r.already) {
        toast('Demo memories are already loaded', 'info');
      } else {
        toast(`Loaded ${r.loaded} demo memories`, 'ok');
      }
      updateStatus();
    } catch (e) { toast(e.message || 'Demo load failed', 'error'); }
  };
  document.getElementById('btn-remove-demo').onclick = async () => {
    if (!confirm('Remove all demo memories? Only memories tagged "demo" are deleted.')) return;
    try {
      const n = await removeDemoMemories();
      toast(`Removed ${n} demo memories`, 'warn');
      updateStatus();
    } catch (e) { toast(e.message || 'Demo removal failed', 'error'); }
  };
  document.getElementById('btn-replay-onboarding').onclick = () => {
    localStorage.removeItem('engram_onboarded');
    showOnboarding();
  };

  // ── Saved searches (watchlist) ──────────────────────────────────────────

  function ssRow(s) {
    return `
      <div class="ss-row" data-id="${esc(s.id)}">
        <span class="ss-query">${esc(s.query || '')}</span>
        ${s.layer ? layerBadge(s.layer) : '<span class="faint">any layer</span>'}
        ${s.notify ? '<span class="badge ok" title="Notify on new matches">🔔</span>' : ''}
        <span class="ss-count faint" data-count>${s.last_match_count ?? '—'} matches</span>
        <button class="note-del ss-del" data-id="${esc(s.id)}" title="Delete saved search">×</button>
      </div>`;
  }

  async function loadSavedSearches() {
    const listEl = document.getElementById('ss-list');
    try {
      const r = await api.savedSearches.list();
      const list = Array.isArray(r) ? r : (r.results || r.searches || []);
      listEl.innerHTML = list.length
        ? list.map(ssRow).join('')
        : '<div class="faint" style="padding:0.75rem 1rem;">No saved searches yet.</div>';
    } catch (e) {
      listEl.innerHTML = '<div class="faint" style="padding:0.75rem 1rem;">Saved searches unavailable.</div>';
    }
  }

  document.getElementById('ss-add').onclick = async () => {
    const query = document.getElementById('ss-query').value.trim();
    if (!query) return;
    const layer = document.getElementById('ss-layer').value || null;
    const notify = document.getElementById('ss-notify').checked;
    try {
      await api.savedSearches.create({ query, layer, notify });
      document.getElementById('ss-query').value = '';
      document.getElementById('ss-notify').checked = false;
      toast('Search saved', 'ok');
      loadSavedSearches();
    } catch (e) {
      toast('Could not save search', 'err');
    }
  };

  updateStatus();
});

// ── Demo memories ──────────────────────────────────────────────────────────
// Guided-tour dataset. Everything is captured with project "demo" (and a
// "demo" tag) so it can be removed in one click from Settings → Privacy
// (purge by project).

const DEMO_MEMORIES = [
  { content: 'Had coffee with Sarah — she wants to try the new project-management tool next week', layer: 'episodic', source: 'interaction', tags: ['people', 'sarah'], valence: 0.6 },
  { content: 'Spent the morning debugging the login page — turned out to be a timezone issue', layer: 'episodic', source: 'window', tags: ['work', 'debugging'], valence: -0.4 },
  { content: 'I prefer concise answers with code examples over long explanations', layer: 'semantic', source: 'interaction', tags: ['preferences'], valence: 0.3 },
  { content: 'The staging server deploys every Tuesday at 10am — never deploy on a Friday', layer: 'semantic', source: 'agent', tags: ['work', 'deploys', 'rule'], valence: 0 },
  { content: 'Asked the assistant to plan a birthday dinner for 8 people — shortlisted three restaurants', layer: 'episodic', source: 'chat', tags: ['personal', 'planning'], valence: 0.5 },
  { content: 'Always book vegetarian options for team dinners — two teammates are vegetarian', layer: 'semantic', source: 'research', tags: ['team', 'food', 'rule'], valence: 0.2 },
  { content: 'What if the calendar warned me before I book meetings during my low-energy afternoons?', layer: 'imagined', source: 'system', tags: ['idea', 'calendar'], valence: 0.1 },
  { content: 'A travel mode that batches non-urgent notifications while abroad', layer: 'imagined', source: 'system', tags: ['idea', 'notifications'], valence: 0.2 },
  { content: 'Thursday evening is usually gym time — skipped it this week for a deadline', layer: 'episodic', source: 'system', tags: ['health', 'routine'], valence: -0.2 },
  { content: 'Pattern noticed: the most productive deep-work sessions happen on Tuesday and Thursday mornings', layer: 'semantic', source: 'consolidation', tags: ['pattern', 'productivity'], valence: 0.4 },
  { content: 'Finished reading "Thinking in Systems" — took notes on feedback loops', layer: 'episodic', source: 'interaction', tags: ['reading', 'books'], valence: 0.5 },
  { content: 'The feedback loops from that book are exactly how memory decay works in this vault', layer: 'semantic', source: 'research', tags: ['reading', 'memory'], valence: 0.3 },
];

// Pairs of indices into DEMO_MEMORIES to connect, so the Graph screen has edges.
const DEMO_LINKS = [
  [4, 5, 'causal'],       // dinner planning → vegetarian rule
  [10, 11, 'analogical'], // systems book → memory decay insight
  [8, 9, 'associative'],  // gym routine → deep-work pattern
  [0, 4, 'associative'],  // Sarah → dinner planning
];

async function loadDemoMemories() {
  // Guard against duplicates — don't load twice
  const existing = await api.memories.search({ sort_by: 'recency', limit: 200 });
  if ((existing.results || []).some(m => (m.tags || []).includes('demo'))) {
    return { loaded: 0, already: true };
  }
  const ids = [];
  for (const d of DEMO_MEMORIES) {
    const r = await post('/memories', {
      content: d.content, layer: d.layer, source: d.source,
      tags: [...d.tags, 'demo'], valence: d.valence, project: 'demo',
    });
    ids.push(r.id);
  }
  for (const [a, b, type] of DEMO_LINKS) {
    if (ids[a] && ids[b]) {
      try { await post('/memories/link', { source_id: ids[a], target_id: ids[b], link_type: type, weight: 0.7 }); } catch (e) { /* links are decorative */ }
    }
  }
  return { loaded: ids.length, already: false };
}

async function removeDemoMemories() {
  const r = await api.privacy.purge({ project: 'demo' });
  return r.deleted ?? r.purged ?? 0;
}

// ── Onboarding wizard ──────────────────────────────────────────────────────

function showOnboarding() {
  const root = document.getElementById('modal-root');
  let step = 1;
  let capturedContent = '';
  let demoLoaded = false;
  const STEPS = 5;

  function close() {
    localStorage.setItem('engram_onboarded', '1');
    root.innerHTML = '';
    updateStatus();
  }

  function dots() {
    return `<div class="ob-dots">${Array.from({ length: STEPS }, (_, i) =>
      `<span class="ob-dot${i + 1 === step ? ' active' : ''}"></span>`).join('')}</div>`;
  }

  function shell(content, actions) {
    root.innerHTML = `
      <div class="ob-overlay">
        <div class="ob-modal">
          ${dots()}
          ${content}
          <div class="ob-actions">${actions}</div>
        </div>
      </div>`;
  }

  function renderStep() {
    if (step === 1) {
      shell(`
        <h3>Welcome — this is your AI's memory</h3>
        <p>AI assistants forget everything the moment a conversation ends. Engram changes that:
        it gives your AI a <strong>long-term memory</strong>, so it can remember your preferences,
        past conversations, and what you were working on.</p>
        <p>This app is the window into that memory. Everything you see here is stored
        <strong>on this computer only</strong>, inside an encrypted vault. Nothing is uploaded
        or shared — you can browse, search, and delete it all.</p>
        <p>Over the next few steps we'll show you how it works. No technical knowledge needed.</p>`,
        `<button class="btn" id="ob-skip">Skip tour</button>
         <button class="btn btn-primary" id="ob-next">How it works →</button>`);
      document.getElementById('ob-skip').onclick = close;
      document.getElementById('ob-next').onclick = () => { step = 2; renderStep(); };

    } else if (step === 2) {
      shell(`
        <h3>Three kinds of memory</h3>
        <p>Just like people, your AI keeps different <em>kinds</em> of memories, shown in
        different colors throughout the app:</p>
        <div class="ob-legend">
          <div>${layerIcon('episodic')} <strong>Episodic — things that happened.</strong><br>
          <span class="faint">Like a journal: "Deployed the website on Tuesday", "Had coffee with Sarah".</span></div>
          <div>${layerIcon('semantic')} <strong>Semantic — things that were learned.</strong><br>
          <span class="faint">Facts, preferences and rules: "You prefer short answers", "Never deploy on Fridays".</span></div>
          <div>${layerIcon('imagined')} <strong>Imagined — ideas the AI came up with.</strong><br>
          <span class="faint">Suggestions it dreamed up on its own. These stay <em>quarantined</em> — clearly marked and never treated as fact — until you approve ("ground") them.</span></div>
        </div>
        <p>Two more things you'll see on every memory:</p>
        <div class="ob-legend">
          <div><strong>Strength</strong> — memories fade when unused and grow stronger when they prove useful, mirroring how human memory works.</div>
          <div><strong>Valence</strong> — the emotional tone, from challenging to joyful, so your AI knows which experiences went well.</div>
        </div>`,
        `<button class="btn" id="ob-back">← Back</button>
         <button class="btn btn-primary" id="ob-next">See it in action →</button>`);
      document.getElementById('ob-back').onclick = () => { step = 1; renderStep(); };
      document.getElementById('ob-next').onclick = () => { step = 3; renderStep(); };

    } else if (step === 3) {
      shell(`
        <h3>Look around with demo memories</h3>
        <p>The easiest way to understand Engram is to see it full. We can load
        <strong>12 ready-made demo memories</strong> — everyday examples like dinner plans,
        work rules, and a few AI-generated ideas — already linked together so the
        <strong>Graph</strong> view lights up.</p>
        <p>They're completely fake, clearly marked with a <span class="mono">demo</span> tag,
        and you can remove every one of them with a single click in
        <strong>Settings → Onboarding &amp; Demo Data</strong> whenever you're done exploring.</p>
        <div id="ob-demo-result" class="ob-result">
          <button class="btn btn-primary" id="ob-load-demo">Load demo memories</button>
        </div>
        <p class="faint">After the tour, open <strong>Explorer</strong> to browse them and
        <strong>Graph</strong> to see how they connect — or skip this and use your own data.</p>`,
        `<button class="btn" id="ob-back">← Back</button>
         <button class="btn" id="ob-next">Skip demo →</button>`);
      document.getElementById('ob-back').onclick = () => { step = 2; renderStep(); };
      document.getElementById('ob-next').onclick = () => { step = 4; renderStep(); };
      document.getElementById('ob-load-demo').onclick = async () => {
        const out = document.getElementById('ob-demo-result');
        out.innerHTML = '<span class="faint">Loading demo memories…</span>';
        try {
          const r = await loadDemoMemories();
          if (r.already) {
            out.innerHTML = '<span class="ok">✓ Demo memories are already loaded.</span> <span class="faint">Remove them from Settings when you\'re done.</span>';
          } else {
            demoLoaded = true;
            out.innerHTML = `<span class="ok">✓ ${r.loaded} demo memories loaded.</span> <span class="faint">They'll show up everywhere in the app.</span>`;
            toast(`${r.loaded} demo memories loaded`, 'ok');
          }
          updateStatus();
          setTimeout(() => { step = 4; renderStep(); }, 1200);
        } catch (e) {
          out.innerHTML = `<span class="error">Couldn't load demo memories: ${esc(e.message)}</span>`;
        }
      };

    } else if (step === 4) {
      shell(`
        <h3>Capture a memory of your own</h3>
        <p>Normally your tools remember things for you automatically. But you can also
        teach your AI something directly — try it now. Anything works:</p>
        <div class="ob-legend">
          <div class="faint">• A preference: "I prefer dark mode and short answers"</div>
          <div class="faint">• A fact: "My team's standup is at 9:30 on Mondays"</div>
          <div class="faint">• Something that happened: "Started the garden project this weekend"</div>
        </div>
        <textarea id="ob-content" class="ob-textarea" rows="3" placeholder="Something worth remembering… (or leave empty to skip)"></textarea>
        <input type="text" id="ob-tags" class="search-input" placeholder="Tags, comma-separated — optional labels for finding this later" style="margin-top:0.5rem;">
        <div id="ob-capture-result" class="ob-result"></div>`,
        `<button class="btn" id="ob-back">← Back</button>
         <button class="btn btn-primary" id="ob-next">Save &amp; continue →</button>`);
      document.getElementById('ob-back').onclick = () => { step = 3; renderStep(); };
      document.getElementById('ob-next').onclick = async () => {
        const content = document.getElementById('ob-content').value.trim();
        const tags = document.getElementById('ob-tags').value.split(',').map(t => t.trim()).filter(Boolean);
        if (!content) { step = 5; renderStep(); return; }
        try {
          const r = await post('/memories', { content, tags: tags.length ? tags : undefined, source: 'interaction' });
          capturedContent = content;
          document.getElementById('ob-capture-result').innerHTML =
            `<span class="ok">✓ Remembered.</span> <span class="faint">Your AI can recall this from now on.</span>`;
          toast('Memory captured', 'ok');
          setTimeout(() => { step = 5; renderStep(); }, 900);
        } catch (e) {
          toast(e.message || 'Capture failed', 'error');
        }
      };

    } else {
      shell(`
        <h3>What your AI actually sees</h3>
        <p>Here's the payoff. When you talk to your AI, Engram quietly picks the most
        relevant memories and slips them into the instructions the AI receives —
        that's how it "remembers".</p>
        <p>Press the button to see the exact message your AI would get
        ${demoLoaded || capturedContent ? 'right now, with your memories in it' : 'once you have some memories'}:</p>
        <div id="ob-context-preview" class="ob-result">
          <button class="btn" id="ob-assemble">Show me what my AI receives</button>
        </div>
        <p class="faint">You can do this any time on the <strong>Context</strong> screen.
        That's it — explore the Dashboard, browse the Explorer, and check the Graph.
        Everything is yours, and everything can be deleted from Settings.</p>`,
        `<button class="btn" id="ob-back">← Back</button>
         <button class="btn btn-primary" id="ob-finish">Start exploring →</button>`);
      document.getElementById('ob-back').onclick = () => { step = 4; renderStep(); };
      document.getElementById('ob-finish').onclick = () => { close(); navigate('#/'); };
      document.getElementById('ob-assemble').onclick = async () => {
        const out = document.getElementById('ob-context-preview');
        out.innerHTML = '<span class="faint">Assembling…</span>';
        try {
          const r = await api.context.assemble({ query: capturedContent || 'What do you remember about me?' });
          out.innerHTML = `
            <div class="faint" style="margin-bottom:0.5rem;">${r.metadata?.total_tokens || 0} tokens · ${r.metadata?.engrams_retrieved || 0} memories included</div>
            <div class="messages-preview">${(r.messages || []).map(m =>
              `<div class="msg-row"><span class="msg-role">[${m.role}]</span> ${esc((m.content || '').slice(0, 300))}${(m.content || '').length > 300 ? '…' : ''}</div>`).join('')}
            </div>`;
        } catch (e) {
          out.innerHTML = `<span class="error">${esc(e.message)}</span>`;
        }
      };
    }
  }

  renderStep();
}

// ── Tour & Demo section ────────────────────────────────────────────────────
// A full page (not a modal) with the plain-language explanation and the live
// demo, accessible at any time from the nav and the "?" topbar button.

route('/tour', async () => {
  const app = document.getElementById('app');

  // Detect whether demo memories are currently loaded
  let demoCount = 0;
  try {
    const s = await api.memories.search({ sort_by: 'recency', limit: 200 });
    demoCount = (s.results || []).filter(m => (m.tags || []).includes('demo')).length;
  } catch (e) { /* API unreachable — demoCount stays 0 */ }

  app.innerHTML = `
    <div class="page tour-page">
      <div class="tour-hero">
        <h2>Your AI forgets everything. <span class="accent">Engram remembers.</span></h2>
        <p>This is a tour of your AI's memory. Read the explanation, then make the app
        come alive with demo memories — fake, clearly marked, and removable in one click.</p>
      </div>

      <div class="panel-grid">
        <div class="panel">
          <div class="panel-header">How it works</div>
          <div class="tour-body">
            <p>AI assistants forget everything the moment a conversation ends. Engram gives
            your AI a <strong>long-term memory</strong> — it quietly records the important
            moments, and recalls them in later conversations.</p>
            <p>Everything is stored <strong>on this computer only</strong>, inside an encrypted
            vault. Nothing is uploaded or shared. You can browse, search, and delete it all.</p>

            <h4>Three kinds of memory</h4>
            <div class="ob-legend">
              <div>${layerIcon('episodic')} <strong>Episodic — things that happened.</strong><br>
              <span class="faint">Like a journal: "Deployed the website on Tuesday", "Had coffee with Sarah".</span></div>
              <div>${layerIcon('semantic')} <strong>Semantic — things that were learned.</strong><br>
              <span class="faint">Facts, preferences and rules: "You prefer short answers", "Never deploy on Fridays".</span></div>
              <div>${layerIcon('imagined')} <strong>Imagined — ideas the AI came up with.</strong><br>
              <span class="faint">Suggestions it dreamed up on its own. These stay <em>quarantined</em> — clearly
              marked and never treated as fact — until you approve ("ground") them.</span></div>
            </div>

            <h4>Two more things on every memory</h4>
            <div class="ob-legend">
              <div><strong>Strength</strong> — memories fade when unused and grow stronger when they prove useful,
              mirroring how human memory works.</div>
              <div><strong>Valence</strong> — the emotional tone, from challenging to joyful, so your AI knows which
              experiences went well.</div>
            </div>

            <p class="faint" style="margin-top:1rem;">Prefer a guided walkthrough?
            <a href="#/tour" id="tour-replay-wizard" class="link">Open the step-by-step wizard</a> instead.</p>
          </div>
        </div>

        <div class="panel">
          <div class="panel-header">Live demo</div>
          <div class="tour-body">
            <p>The easiest way to understand Engram is to see it full. Load
            <strong>12 ready-made demo memories</strong> — everyday examples like dinner plans,
            work rules, and a few AI-generated ideas — already linked together so the
            <strong>Graph</strong> view lights up.</p>
            <p>They're completely fake, clearly marked with a <span class="mono">demo</span> tag,
            and every one of them can be removed with a single click.</p>

            <div class="tour-demo-state" id="tour-demo-state">
              ${demoCount > 0
                ? `<span class="ok">● ${demoCount} demo memories are currently loaded</span>
                   <span class="faint">They appear in the Explorer alongside your real memories.</span>`
                : `<span class="faint">No demo memories loaded right now.</span>`}
            </div>

            <div class="tour-demo-actions">
              <button class="btn btn-primary" id="tour-load-demo">Load demo memories</button>
              <button class="btn" id="tour-remove-demo" ${demoCount > 0 ? '' : 'disabled'}>Remove demo memories</button>
            </div>
            <div id="tour-demo-result" class="ob-result"></div>

            <h4>Where to look</h4>
            <div class="tour-demo-actions">
              <a class="btn" href="#/memories">Explorer — browse them</a>
              <!-- Graph CTA parked as a future idea (2026-08-14); see index.html nav comment -->
              <a class="btn" href="#/context">Context — what your AI receives</a>
            </div>
          </div>
        </div>

        <div class="panel">
          <div class="panel-header">Try it yourself</div>
          <div class="tour-body">
            <p>Normally your tools remember things for you automatically. But you can also
            teach your AI something directly — anything works:</p>
            <div class="ob-legend">
              <div class="faint">• A preference: "I prefer dark mode and short answers"</div>
              <div class="faint">• A fact: "My team's standup is at 9:30 on Mondays"</div>
              <div class="faint">• Something that happened: "Started the garden project this weekend"</div>
            </div>
            <textarea id="tour-content" class="ob-textarea" rows="3" placeholder="Something worth remembering…"></textarea>
            <input type="text" id="tour-tags" class="search-input" placeholder="Tags, comma-separated — optional labels for finding this later" style="margin-top:0.5rem;">
            <div class="tour-demo-actions" style="margin-top:0.75rem;">
              <button class="btn btn-primary" id="tour-capture">Remember this</button>
            </div>
            <div id="tour-capture-result" class="ob-result"></div>

            <h4>What your AI sees</h4>
            <p class="faint">Engram quietly picks the most relevant memories and slips them into the
            instructions your AI receives — that's how it "remembers". Press the button to see
            the exact message your AI would get:</p>
            <div class="tour-demo-actions">
              <button class="btn" id="tour-assemble">Show me what my AI receives</button>
            </div>
            <div id="tour-context-preview" class="ob-result"></div>
          </div>
        </div>
      </div>
    </div>`;

  // ── Wizard replay link
  document.getElementById('tour-replay-wizard').onclick = (e) => {
    e.preventDefault();
    showOnboarding();
  };

  // ── Demo load / remove
  document.getElementById('tour-load-demo').onclick = async () => {
    const out = document.getElementById('tour-demo-result');
    out.innerHTML = '<span class="faint">Loading demo memories…</span>';
    try {
      const r = await loadDemoMemories();
      if (r.already) {
        out.innerHTML = '<span class="ok">✓ Demo memories are already loaded.</span>';
      } else {
        out.innerHTML = `<span class="ok">✓ ${r.loaded} demo memories loaded.</span> <span class="faint">They'll show up everywhere in the app.</span>`;
        toast(`${r.loaded} demo memories loaded`, 'ok');
      }
      updateStatus();
      // Refresh the demo-state line without a full re-render
      const state = document.getElementById('tour-demo-state');
      if (state) state.innerHTML = `<span class="ok">● ${r.loaded || 12} demo memories are currently loaded</span>
        <span class="faint">They appear in the Explorer and Graph alongside your real memories.</span>`;
      const rm = document.getElementById('tour-remove-demo');
      if (rm) rm.disabled = false;
    } catch (e) {
      out.innerHTML = `<span class="error">Couldn't load demo memories: ${esc(e.message)}</span>`;
    }
  };

  document.getElementById('tour-remove-demo').onclick = async () => {
    const out = document.getElementById('tour-demo-result');
    out.innerHTML = '<span class="faint">Removing demo memories…</span>';
    try {
      const n = await removeDemoMemories();
      out.innerHTML = `<span class="ok">✓ Removed ${n} demo memories.</span>`;
      toast(`Removed ${n} demo memories`, 'ok');
      updateStatus();
      const state = document.getElementById('tour-demo-state');
      if (state) state.innerHTML = '<span class="faint">No demo memories loaded right now.</span>';
      document.getElementById('tour-remove-demo').disabled = true;
    } catch (e) {
      out.innerHTML = `<span class="error">Couldn't remove demo memories: ${esc(e.message)}</span>`;
    }
  };

  // ── Capture
  document.getElementById('tour-capture').onclick = async () => {
    const content = document.getElementById('tour-content').value.trim();
    if (!content) { toast('Type something worth remembering first', 'info'); return; }
    const tags = document.getElementById('tour-tags').value.split(',').map(t => t.trim()).filter(Boolean);
    const out = document.getElementById('tour-capture-result');
    out.innerHTML = '<span class="faint">Saving…</span>';
    try {
      const r = await post('/memories', { content, tags: tags.length ? tags : undefined, source: 'interaction' });
      document.getElementById('tour-content').value = '';
      document.getElementById('tour-tags').value = '';
      out.innerHTML = `<span class="ok">✓ Remembered.</span> <span class="faint">Your AI can recall this from now on.</span>`;
      toast('Memory captured', 'ok');
      updateStatus();
    } catch (e) {
      out.innerHTML = `<span class="error">${esc(e.message)}</span>`;
    }
  };

  // ── Context preview
  document.getElementById('tour-assemble').onclick = async () => {
    const out = document.getElementById('tour-context-preview');
    out.innerHTML = '<span class="faint">Assembling…</span>';
    try {
      const r = await api.context.assemble({ query: 'What do you remember about me?' });
      out.innerHTML = `
        <div class="faint" style="margin-bottom:0.5rem;">${r.metadata?.total_tokens || 0} tokens · ${r.metadata?.engrams_retrieved || 0} memories included</div>
        <div class="messages-preview">${(r.messages || []).map(m =>
          `<div class="msg-row"><span class="msg-role">[${m.role}]</span> ${esc((m.content || '').slice(0, 300))}${(m.content || '').length > 300 ? '…' : ''}</div>`).join('')}
        </div>`;
    } catch (e) {
      out.innerHTML = `<span class="error">${esc(e.message)}</span>`;
    }
  };
});

// ── Init ──────────────────────────────────────────────────────────────────

updateStatus();
setInterval(updateStatus, 30000);

// Tour & demo: accessible at any time from the topbar
document.getElementById('tour-btn').addEventListener('click', () => navigate('#/tour'));

// Quick capture: topbar button + global hotkey (Ctrl/Cmd+K). Mobile stays
// browse-only, so both entry points no-op under the mobile flag.
const openCapture = () => {
  if (document.body.classList.contains('mobile')) return;
  openCaptureModal();
};
document.getElementById('capture-btn').addEventListener('click', openCapture);
window.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    openCapture();
  }
});

// Modal close — delegated listener (CSP blocks inline event-handler attributes,
// so closing happens here instead of onclick="" in showModal)
document.getElementById('modal-root').addEventListener('click', (e) => {
  if (e.target.classList.contains('modal-overlay') || e.target.classList.contains('modal-close')) {
    document.getElementById('modal-root').innerHTML = '';
  }
});

// Import file picker — delegated change listener (CSP-safe replacement for the
// former inline onchange; change events bubble)
document.addEventListener('change', (e) => {
  if (e.target && e.target.id === 'import-file') {
    document.getElementById('import-btn').style.display = 'inline-block';
  }
});

// Mobile read-only flag (K3: mobile is browse-only)
const mobileMq = window.matchMedia('(max-width: 767px)');
const applyMobileFlag = () => document.body.classList.toggle('mobile', mobileMq.matches);
if (mobileMq.addEventListener) mobileMq.addEventListener('change', applyMobileFlag);
applyMobileFlag();

// Onboarding: show on first visit, or whenever the vault is empty
(async () => {
  const onboarded = localStorage.getItem('engram_onboarded');
  let empty = false;
  try {
    const s = await api.stats();
    empty = (s.total_memories || 0) === 0;
  } catch (e) {
    return; // server down — don't onboard against a dead API
  }
  if (!onboarded || empty) showOnboarding();
})();
