// ==========================================================================
// Engram Memory Vault — SPA
// ==========================================================================

import { MemoryGraph } from './graph.js';

const API = '';  // relative to origin — Caddy reverse-proxies to localhost:8787

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
  const r = await fetch(API + path);
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  return r.json();
}

async function post(path, body = {}) {
  const r = await fetch(API + path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  return r.json();
}

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
    delete: (id) => fetch(API + '/memories/' + id, { method: 'DELETE' }),
    annotations: (id) => get('/memories/' + id + '/annotations'),
    annotate: (id, content) => post('/memories/' + id + '/annotations', { content }),
  },
  annotations: {
    delete: (id) => fetch(API + '/annotations/' + id, { method: 'DELETE' }),
  },
  analytics: {
    activity: (days = 30) => get('/analytics/activity?days=' + days),
    co2: () => get('/analytics/co2'),
  },
  savedSearches: {
    list: () => get('/searches'),
    create: (s) => post('/searches', s),
    delete: (id) => fetch(API + '/searches/' + id, { method: 'DELETE' }),
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
    update: (c) => fetch(API + '/config', { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(c) }),
  },
};

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
        app.innerHTML = `<div class="error-panel"><p>Error: ${esc(e.message)}</p></div>`;
      }
      return;
    }
  }
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

// ==========================================================================
// SCREENS
// ==========================================================================

// ── Dashboard ─────────────────────────────────────────────────────────────

route('/', async () => {
  let stats, health, activity, co2, history;
  try { stats = await api.stats(); } catch (e) { stats = null; }

  const app = document.getElementById('app');

  if (!stats) {
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
      await api.memories.create({ content, layer: 'episodic', source: 'interaction' });
      ta.value = '';
      toast('Captured', 'ok');
      // Pull the fresh memory into the feed (WS will dedupe by id)
      const r = await api.memories.search({ sort_by: 'recency', limit: 3 });
      const list = Array.isArray(r) ? r : (r.results || []);
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

// ── Settings ───────────────────────────────────────────────────────────────

route('/settings', async () => {
  const app = document.getElementById('app');
  let config, audit;
  try { config = await api.config.get(); } catch (e) { config = { vault_path: '~/.engram/vaults/default', encryption: 'sqlcipher' }; }
  try { audit = await api.privacy.audit(); } catch (e) { audit = null; }

  const ctx = config.context || {};
  const sched = config.schedule || {};
  const emb = config.embedding || {};
  const breakdown = audit?.breakdown || {};

  const breakdownRows = (items, key, iconFn) => (items || []).length
    ? items.map(i => `<div class="health-row">${iconFn ? iconFn(i[key]) + ' ' : ''}${esc(i[key] || '—')}<span class="ml-auto mono">${i.count}</span></div>`).join('')
    : '<div class="health-row faint">No data.</div>';

  app.innerHTML = `
    <div class="page settings-page">
      <h2>Vault Settings</h2>

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

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Remote Access</div>
        <div class="settings-note">
          API key authentication is configured server-side via the <code>ENGRAMD_API_KEY</code>
          environment variable when <code>engramd</code> starts. It cannot be changed from the UI.
          See <code>docs/engram-product/DEPLOY.md</code> for exposing the vault behind Caddy.
        </div>
      </div>

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
  `;

  // ── Slider value labels
  document.getElementById('set-budget').oninput = function() {
    document.getElementById('set-budget-val').textContent = this.value;
  };
  document.getElementById('set-reserve').oninput = function() {
    document.getElementById('set-reserve-val').textContent = this.value + '%';
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
                   <span class="faint">They appear in the Explorer and Graph alongside your real memories.</span>`
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
              <a class="btn" href="#/graph">Graph — see them connect</a>
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
