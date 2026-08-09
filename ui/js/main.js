// ==========================================================================
// Engram Memory Vault — SPA
// ==========================================================================

const API = '';  // relative to origin — Caddy reverse-proxies to localhost:8787

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
    links: (id) => get('/memories/' + id + '/links'),
    related: (id, limit = 10) => get('/memories/' + id + '/related?limit=' + limit),
    ground: (id) => post('/memories/' + id + '/ground'),
    delete: (id) => fetch(API + '/memories/' + id, { method: 'DELETE' }),
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

// ── Status bar ────────────────────────────────────────────────────────────

async function updateStatus() {
  try {
    const h = await api.health();
    const el = document.getElementById('statusbar');
    el.innerHTML = `
      <span class="status-item ok">● Connected</span>
      <span class="status-item">${h.memories_total || 0} memories</span>
      <span class="status-item">QEM ${Math.round((h.qem_hit_rate || 0) * 100)}%</span>
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

// ── Modal ─────────────────────────────────────────────────────────────────

function showModal(title, bodyHtml) {
  const root = document.getElementById('modal-root');
  root.innerHTML = `
    <div class="modal-overlay" onclick="this.closest('#modal-root').innerHTML=''">
      <div class="modal" onclick="event.stopPropagation()">
        <div class="modal-header">
          <h2>${title}</h2>
          <button class="modal-close" onclick="this.closest('#modal-root').innerHTML=''">&times;</button>
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
  let stats, health;
  try { stats = await api.stats(); } catch (e) { stats = null; }
  try { health = await api.health(); } catch (e) { health = null; }

  const app = document.getElementById('app');

  if (!stats) {
    app.innerHTML = `<div class="error-panel">
      <h2>Cannot reach engramd</h2>
      <p>Make sure <code>python3 server.py</code> is running on port 8787.</p>
    </div>`;
    return;
  }

  const layerPct = (n) => stats.total_memories ? ((n / stats.total_memories) * 100).toFixed(1) : 0;

  app.innerHTML = `
    <div class="page dashboard">
      <div class="stat-grid">
        <div class="stat-card">
          <div class="stat-num">${stats.total_memories?.toLocaleString() || 0}</div>
          <div class="stat-label">Memories</div>
        </div>
        <div class="stat-card">
          <div class="stat-num">${Math.round((health?.qem_hit_rate || 0) * 100)}%</div>
          <div class="stat-label">Cache hit rate (QEM)</div>
        </div>
        <div class="stat-card">
          <div class="stat-num">${stats.by_layer?.semantic || 0}</div>
          <div class="stat-label">Semantic memories</div>
        </div>
        <div class="stat-card">
          <div class="stat-num">${stats.by_layer?.imagined || 0}</div>
          <div class="stat-label">Imagined (quarantined)</div>
        </div>
      </div>

      <div class="panel-grid">
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
          </div>
        </div>

        <div class="panel">
          <div class="panel-header">Vault Health</div>
          <div class="health-list">
            <div class="health-row"><span class="ok">●</span> Vault encrypted</div>
            <div class="health-row"><span>${formatBytes(health?.db_size_bytes || 0)}</span> / no limit</div>
            <div class="health-row"><span>${stats.total_links || 0}</span> links</div>
            <div class="health-row"><span>${stats.total_embeddings || 0}</span> embeddings</div>
            <div class="health-row faint">Avg strength: ${(stats.avg_strength || 0).toFixed(2)}</div>
            <div class="health-row faint">Avg valence: ${(stats.avg_valence || 0).toFixed(2)}</div>
          </div>
        </div>
      </div>

      <div class="panel" style="margin-top:1rem;">
        <div class="panel-header">Recent Captures</div>
        <div id="recent-feed">Loading…</div>
      </div>
    </div>
  `;

  // Load recent captures
  try {
    const r = await api.memories.search({ sort_by: 'recency', limit: 5 });
    const feed = document.getElementById('recent-feed');
    if (r.results && r.results.length) {
      feed.innerHTML = r.results.map(m => `
        <a href="#/memories/${m.id}" class="feed-item">
          ${layerIcon(m.layer)} <span class="faint">[${m.source || '?'}]</span>
          ${esc((m.content || '').slice(0, 120))}${(m.content || '').length > 120 ? '…' : ''}
          <span class="faint ml-auto">${ago(m.created_at)}</span>
        </a>
      `).join('');
    } else {
      feed.innerHTML = '<div class="faint" style="padding:1rem;">No memories captured yet.</div>';
    }
  } catch (e) {
    document.getElementById('recent-feed').innerHTML = '<div class="faint">Unable to load recent captures.</div>';
  }

  updateStatus();
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
        <span class="faint" style="margin-left:auto;" id="result-count"></span>
      </div>
      <div id="explorer-results" class="explorer-results"></div>
    </div>
  `;

  function doSearch() {
    const q = document.getElementById('search-input').value;
    const layer = document.getElementById('filter-layer').value;
    const sort = document.getElementById('filter-sort').value;
    const results = document.getElementById('explorer-results');
    results.innerHTML = '<div class="loading-sm">Searching…</div>';
    api.memories.search({ query: q, layer: layer || null, sort_by: sort, limit: 20 }).then(r => {
      document.getElementById('result-count').textContent = `${r.total || 0} results (${resultLabel(r.search_type)}, ${r.took_ms}ms)`;
      if (!r.results || !r.results.length) {
        results.innerHTML = '<div class="faint" style="padding:1rem;">No results.</div>';
        return;
      }
      results.innerHTML = r.results.map(m => `
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
            ${m.links_out && m.links_out.length ? `<span class="faint">→ ${m.links_out.length} links</span>` : ''}
            ${m.imagined && !m.grounded ? '<span class="badge badge-quarantined">⚠ quarantined</span>' : ''}
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
      if (l.outgoing && l.outgoing.length) {
        linksHtml = `
          <div class="panel" style="margin-top:1rem;">
            <div class="panel-header">Links (${l.outgoing.length} outgoing)</div>
            ${l.outgoing.map(ln => `
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
        ${linksHtml}
        <div class="detail-actions">
          ${m.imagined && !m.grounded ? '<button class="btn" id="btn-ground">Ground memory</button>' : ''}
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
    }
    document.getElementById('btn-delete').onclick = async () => {
      if (confirm('Delete this memory permanently?')) {
        await api.memories.delete(id);
        toast('Deleted', 'warn');
        navigate('#/memories');
      }
    };
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
        <input type="text" id="gf-search" class="search-input" placeholder="Focus on a memory ID…" style="width:260px;">
        <button class="btn btn-sm" id="gf-search-btn">Focus</button>
        <button class="btn btn-sm" id="gf-reset">Reset</button>
      </div>
      <div id="graph-canvas" class="graph-canvas">
        <div class="faint" style="padding:3rem;text-align:center;">Loading graph data…</div>
      </div>
      <div id="graph-legend" class="graph-legend">
        <span>${layerIcon('episodic')} Episodic</span>
        <span>${layerIcon('semantic')} Semantic</span>
        <span>${layerIcon('imagined')} Imagined</span>
        <span class="faint">|</span>
        <span style="border-bottom:2px dotted var(--text-faint);">Assoc</span>
        <span style="border-bottom:2px solid var(--accent);">Causal</span>
        <span style="border-bottom:2px dashed var(--text-faint);">Analog</span>
        <span style="border-bottom:2px dotted var(--episodic);">Temp</span>
      </div>
    </div>
  `;

  async function loadGraph(filterLayer, focusId) {
    const canvas = document.getElementById('graph-canvas');
    try {
      const r = await api.memories.search({
        query: focusId ? '' : '',
        layer: filterLayer || null,
        sort_by: 'strength',
        limit: 50,
        min_strength: 0.2
      });
      if (!r.results || !r.results.length) {
        canvas.innerHTML = '<div class="faint" style="padding:3rem;text-align:center;">No memories to graph.</div>';
        return;
      }

      const nodes = r.results.map(m => ({
        id: m.id,
        label: (m.content || m.id).slice(0, 60),
        layer: m.layer,
        strength: m.strength || 0.5,
        valence: m.valence || 0,
      }));

      // Fetch links for first 10 nodes (avoid N+1 storm)
      const edges = [];
      for (const n of nodes.slice(0, 10)) {
        try {
          const l = await api.memories.links(n.id);
          if (l.outgoing) {
            for (const ln of l.outgoing) {
              if (nodes.find(x => x.id === ln.target_id)) {
                edges.push({ source: n.id, target: ln.target_id, type: ln.link_type, weight: ln.weight });
              }
            }
          }
        } catch (e) { /* skip */ }
      }

      renderGraph(canvas, nodes, edges, focusId);
    } catch (e) {
      canvas.innerHTML = `<div class="error">${esc(e.message)}</div>`;
    }
  }

  document.getElementById('gf-search-btn').onclick = () => {
    const id = document.getElementById('gf-search').value.trim();
    loadGraph(document.getElementById('gf-layer').value, id || null);
  };
  document.getElementById('gf-reset').onclick = () => {
    document.getElementById('gf-search').value = '';
    loadGraph('', null);
  };
  document.getElementById('gf-layer').onchange = () => {
    loadGraph(document.getElementById('gf-layer').value, document.getElementById('gf-search').value.trim() || null);
  };

  await loadGraph('', null);
  updateStatus();
});

function renderGraph(container, nodes, edges, focusId) {
  const w = container.clientWidth || 800;
  const h = 500;
  const cx = w / 2, cy = h / 2;

  // Place nodes in a circle initially
  const angle = (2 * Math.PI) / nodes.length;
  nodes.forEach((n, i) => {
    const r = Math.min(w, h) * 0.35;
    n.x = cx + r * Math.cos(angle * i);
    n.y = cy + r * Math.sin(angle * i);
    n.vx = 0; n.vy = 0;
  });

  // Simple force layout (50 iterations)
  for (let iter = 0; iter < 50; iter++) {
    // Repulsion between all pairs
    for (let i = 0; i < nodes.length; i++) {
      for (let j = i + 1; j < nodes.length; j++) {
        const dx = nodes[j].x - nodes[i].x;
        const dy = nodes[j].y - nodes[i].y;
        const d = Math.max(1, Math.sqrt(dx * dx + dy * dy));
        const f = 2000 / (d * d);
        const fx = (dx / d) * f;
        const fy = (dy / d) * f;
        nodes[i].vx -= fx; nodes[i].vy -= fy;
        nodes[j].vx += fx; nodes[j].vy += fy;
      }
    }
    // Attraction along edges
    for (const e of edges) {
      const s = nodes.find(n => n.id === e.source);
      const t = nodes.find(n => n.id === e.target);
      if (!s || !t) continue;
      const dx = t.x - s.x;
      const dy = t.y - s.y;
      const d = Math.max(1, Math.sqrt(dx * dx + dy * dy));
      const f = d * 0.01 * e.weight;
      s.vx += (dx / d) * f; s.vy += (dy / d) * f;
      t.vx -= (dx / d) * f; t.vy -= (dy / d) * f;
    }
    // Center gravity
    for (const n of nodes) {
      n.vx += (cx - n.x) * 0.001;
      n.vy += (cy - n.y) * 0.001;
    }
    // Apply velocity with damping
    for (const n of nodes) {
      n.x += n.vx * 0.5;
      n.y += n.vy * 0.5;
      n.vx *= 0.9;
      n.vy *= 0.9;
      n.x = Math.max(40, Math.min(w - 40, n.x));
      n.y = Math.max(20, Math.min(h - 20, n.y));
    }
  }

  const layerColors = { episodic: 'var(--episodic)', semantic: 'var(--semantic)', imagined: 'var(--imagined)' };
  const edgeStyles = { associative: '2,4', causal: '', analogical: '6,3', temporal: '2,4' };
  const edgeColors = { associative: 'var(--text-faint)', causal: 'var(--accent)', analogical: 'var(--text-faint)', temporal: 'var(--episodic)' };

  const edgeSvg = edges.map(e => {
    const s = nodes.find(n => n.id === e.source);
    const t = nodes.find(n => n.id === e.target);
    if (!s || !t) return '';
    const dash = edgeStyles[e.type] || '';
    return `<line x1="${s.x}" y1="${s.y}" x2="${t.x}" y2="${t.y}" stroke="${edgeColors[e.type]}" stroke-width="${1 + e.weight}" stroke-dasharray="${dash}" opacity="0.5"/>`;
  }).join('');

  const nodeSvg = nodes.map(n => {
    const r = 3 + (n.strength || 0.5) * 6;
    const color = layerColors[n.layer] || 'var(--text)';
    const glow = n.id === focusId ? `filter="url(#glow)"` : '';
    return `<g>
      <circle cx="${n.x}" cy="${n.y}" r="${r}" fill="${color}" opacity="0.9" ${glow}>
        <title>${esc(n.label)} (${n.layer})</title>
      </circle>
      <text x="${n.x + r + 4}" y="${n.y + 3}" font-size="10" fill="var(--text-muted)" font-family="var(--mono)">${esc(n.label.slice(0, 30))}</text>
    </g>`;
  }).join('');

  container.innerHTML = `
    <svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" style="display:block;">
      <defs>
        <filter id="glow"><feGaussianBlur stdDeviation="3" result="g"/><feMerge><feMergeNode in="g"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
      </defs>
      <rect width="${w}" height="${h}" fill="var(--bg)"/>
      ${edgeSvg}
      ${nodeSvg}
    </svg>`;
}

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
        <div class="faint" style="margin-bottom:0.5rem;">${r.metadata?.total_tokens || 0} / ${r.metadata?.budget || budget} tokens · ${r.metadata?.engrams_retrieved || 0} engrams · ${r.metadata?.retrieval_took_ms || 0}ms</div>
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
  try { history = await api.consolidate.history(); } catch (e) { history = { runs: [] }; }
  try { stats = await api.stats(); } catch (e) { stats = null; }
  try { patterns = await api.patterns({ query: '', min_engrams: 3 }); } catch (e) { patterns = null; }

  app.innerHTML = `
    <div class="page consolidation-page">
      <h2>Consolidation History</h2>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Run History</div>
        ${(history.runs || []).length ? history.runs.map(r => `
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
  let config;
  try { config = await api.config.get(); } catch (e) { config = { vault_path: '~/.engram/vaults/default', encryption: 'sqlcipher' }; }

  app.innerHTML = `
    <div class="page settings-page">
      <h2>Vault Settings</h2>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Vault</div>
        <div class="health-list">
          <div class="health-row">Name: <span class="mono">default</span></div>
          <div class="health-row">Path: <span class="mono">${esc(config.vault_path || '~/.engram/vaults/default')}</span></div>
          <div class="health-row">Encryption: <span class="ok">${config.encryption || 'sqlcipher'}</span></div>
        </div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Schedule</div>
        <div class="health-list">
          <div class="health-row">Decay: <span>${config.decay_schedule || 'daily'}</span></div>
          <div class="health-row">Consolidation: <span>${config.consolidation_schedule || 'weekly'}</span></div>
        </div>
      </div>

      <div class="panel" style="margin-bottom:1rem;">
        <div class="panel-header">Import / Export</div>
        <div style="display:flex;gap:0.5rem;padding:1rem;">
          <button class="btn" id="btn-export">Export (JSONL)</button>
          <label class="btn" style="cursor:pointer;">
            Import
            <input type="file" id="import-file" accept=".jsonl,.json" style="display:none;" onchange="document.getElementById('import-btn').style.display='inline-block';">
          </label>
          <button class="btn" id="import-btn" style="display:none;">Upload & Import</button>
        </div>
      </div>
    </div>
  `;

  document.getElementById('btn-export').onclick = async () => {
    const r = await api.export({ format: 'jsonl' });
    const blob = new Blob([typeof r === 'string' ? r : JSON.stringify(r)], { type: 'application/x-ndjson' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'engrams-export.jsonl';
    a.click();
    toast('Export complete', 'ok');
  };

  document.getElementById('import-btn').onclick = async () => {
    const file = document.getElementById('import-file').files[0];
    if (!file) return;
    const text = await file.text();
    const r = await api.import(text);
    toast(`Imported ${r.imported || 0} memories`, 'ok');
  };

  updateStatus();
});

// ── Init ──────────────────────────────────────────────────────────────────

updateStatus();
setInterval(updateStatus, 30000);
