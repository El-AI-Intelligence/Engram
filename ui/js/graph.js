// ── MemoryGraph — Canvas-based force-directed memory graph ──────────────────
//
// Features:
//   • Continuous force simulation with Verlet integration + alpha cooling
//   • Canvas 2D rendering with radial-glow nodes and particle-trail edges
//   • Mouse-wheel zoom + drag-to-pan (no D3 dependency)
//   • Click to expand a node (fetches connected memories, animates them in)
//   • Hover tooltip showing memory preview
//   • Focus-animation: smooth camera fly + pulse highlight
//   • Layer-specific visual treatment (episodic=warm, semantic=crystalline, imagined=ethereal)
//   • Responsive — fills container, handles resize
//
// Exports: MemoryGraph class. Instantiate per graph page.

const LAYER_COLORS = {
  episodic:  { fill: '#f0a040', glow: '#f0a040', ring: '#f0c060' },
  semantic:  { fill: '#48c0e0', glow: '#48c0e0', ring: '#68d8f8' },
  imagined:  { fill: '#b080e0', glow: '#b080e0', ring: '#c8a0f8' },
};
const LAYER_ICONS  = { episodic: '◉', semantic: '◆', imagined: '◇' };
const EDGE_STYLES  = {
  associative:  { dash: [4, 5],   color: '#667788', particles: 2 },
  causal:       { dash: [],       color: '#48c0e0', particles: 4 },
  analogical:   { dash: [8, 4],   color: '#889999', particles: 1 },
  temporal:     { dash: [3, 6],   color: '#f0a040', particles: 3 },
};

export class MemoryGraph {
  constructor(container) {
    this.container = container;
    this.canvas   = null;
    this.ctx      = null;

    // Data
    this.nodes    = [];
    this.edges    = [];
    this.nodeMap  = new Map();   // id → node
    this.edgeParticles = [];     // { edge, t, speed }

    // Simulation state
    this.simAlpha     = 0;
    this.simRunning   = false;

    // Viewport
    this.view = { x: 0, y: 0, scale: 1, targetX: 0, targetY: 0, targetScale: 1 };
    this.dragging      = null; // 'pan' | 'node'
    this.dragStart     = { x: 0, y: 0 };
    this.dragNode      = null;
    this.hoverNode     = null;
    this.focusNode     = null;
    this.focusPulse    = 0;    // 0–1 animation phase

    // Tooltip
    this.tooltip       = null;
    this.tooltipTimer  = null;

    // Particle pre-render cache
    this.particleGrad  = null;

    this._bind();
  }

  // ── Public API ──────────────────────────────────────────────────────────

  /** Load graph data: { nodes: [{id,label,layer,strength,valence}], edges: [{source,target,type,weight}] } */
  load(data, focusId) {
    this.nodes = data.nodes.map(n => ({
      ...n,
      x: 0, y: 0, vx: 0, vy: 0,
      radius: 3 + (n.strength || 0.5) * 10,
      expanded: false,
      opacity: 0,
      targetOpacity: 1,
    }));
    this.nodeMap.clear();
    for (const n of this.nodes) this.nodeMap.set(n.id, n);

    this.edges = [];
    for (const e of data.edges) {
      const s = this.nodeMap.get(e.source);
      const t = this.nodeMap.get(e.target);
      if (s && t) this.edges.push({ ...e, sourceNode: s, targetNode: t });
    }

    this._seedParticles();
    this._initPositions();
    this._resetView();
    if (focusId) this._focusNode(focusId);

    // Start simulation
    this.simAlpha = 0.3;
    if (!this.simRunning) { this.simRunning = true; this._tick(); }

    // Fade nodes in
    for (const n of this.nodes) { n.opacity = 0; n.targetOpacity = 1; n._fadeDelay = Math.random() * 0.4; }
  }

  /** Add new nodes + edges (expansion). Animate them outward from the parent node. */
  expand(parentId, newNodes, newEdges) {
    const parent = this.nodeMap.get(parentId);
    if (!parent) return;

    parent.expanded = true;
    const existing = new Set(this.nodes.map(n => n.id));

    for (const nd of newNodes) {
      if (existing.has(nd.id)) {
        // Already present — just make sure it's visible
        const existing = this.nodeMap.get(nd.id);
        existing.targetOpacity = 1;
        continue;
      }
      const node = {
        ...nd,
        x: parent.x + (Math.random() - 0.5) * 20,
        y: parent.y + (Math.random() - 0.5) * 20,
        vx: (Math.random() - 0.5) * 4,
        vy: (Math.random() - 0.5) * 4,
        radius: 3 + (nd.strength || 0.5) * 10,
        expanded: false,
        opacity: 0,
        targetOpacity: 1,
        _fadeDelay: Math.random() * 0.3,
      };
      this.nodes.push(node);
      this.nodeMap.set(node.id, node);
      existing.add(node.id);
    }

    for (const e of newEdges) {
      const s = this.nodeMap.get(e.source);
      const t = this.nodeMap.get(e.target);
      if (s && t && !this.edges.some(ee => ee.source === e.source && ee.target === e.target)) {
        this.edges.push({ ...e, sourceNode: s, targetNode: t });
      }
    }

    this._seedParticles();
    this.simAlpha = Math.max(this.simAlpha, 0.15); // Re-heat simulation
  }

  /** Focus a node by ID — smooth camera fly + pulse. */
  focus(id) { this._focusNode(id); }

  /** Reset view to see all nodes. */
  resetView() { this._resetView(); }

  destroy() {
    this.simRunning = false;
    if (this._resizeObs) this._resizeObs.disconnect();
    this._removeTooltip();
  }

  // ── Initialization ──────────────────────────────────────────────────────

  _bind() {
    // Create canvas
    this.canvas = document.createElement('canvas');
    this.canvas.className = 'mg-canvas';
    this.canvas.style.cssText = 'display:block;width:100%;height:100%;cursor:grab;';
    this.ctx = this.canvas.getContext('2d');
    this.container.innerHTML = '';
    this.container.appendChild(this.canvas);

    // Resize observer
    this._resize();
    this._resizeObs = new ResizeObserver(() => this._resize());
    this._resizeObs.observe(this.container);

    // Mouse events
    this.canvas.addEventListener('mousedown',   e => this._onMouseDown(e));
    this.canvas.addEventListener('mousemove',   e => this._onMouseMove(e));
    this.canvas.addEventListener('mouseup',     e => this._onMouseUp(e));
    this.canvas.addEventListener('mouseleave',  e => this._onMouseUp(e));
    this.canvas.addEventListener('wheel',       e => { e.preventDefault(); this._onWheel(e); }, { passive: false });
    this.canvas.addEventListener('dblclick',    e => { this._resetView(); });
    this.canvas.addEventListener('contextmenu', e => e.preventDefault());
  }

  _resize() {
    const rect = this.container.getBoundingClientRect();
    const dpr  = window.devicePixelRatio || 1;
    const w = rect.width, h = rect.height;
    if (w === 0 || h === 0) return;
    this.canvas.width  = w * dpr;
    this.canvas.height = h * dpr;
    this.canvas.style.width  = w + 'px';
    this.canvas.style.height = h + 'px';
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.W = w; this.H = h;
    this.particleGrad = null; // invalidate cache
  }

  _initPositions() {
    const cx = this.W / 2, cy = this.H / 2;
    const r  = Math.min(this.W, this.H) * 0.35;
    this.nodes.forEach((n, i) => {
      const a = (2 * Math.PI * i) / this.nodes.length - Math.PI / 2;
      n.x = cx + r * Math.cos(a) + (Math.random() - 0.5) * 30;
      n.y = cy + r * Math.sin(a) + (Math.random() - 0.5) * 30;
      n.vx = 0; n.vy = 0;
    });
  }

  _seedParticles() {
    this.edgeParticles = [];
    for (const e of this.edges) {
      const cfg = EDGE_STYLES[e.type] || EDGE_STYLES.associative;
      for (let i = 0; i < (cfg.particles || 2); i++) {
        this.edgeParticles.push({ edge: e, t: Math.random(), speed: 0.0008 + Math.random() * 0.0015 });
      }
    }
  }

  // ── Simulation ──────────────────────────────────────────────────────────

  _tick() {
    if (!this.simRunning) return;
    requestAnimationFrame(() => this._tick());

    const dt = 0.5;
    const nodes = this.nodes;
    const edges = this.edges;
    const alpha = this.simAlpha;

    if (alpha > 0.001) {
      // ── Repulsion (Barnes-Hut would be better; all-pairs is fine for < 200 nodes)
      for (let i = 0; i < nodes.length; i++) {
        for (let j = i + 1; j < nodes.length; j++) {
          let dx = nodes[j].x - nodes[i].x;
          let dy = nodes[j].y - nodes[i].y;
          let d2 = dx * dx + dy * dy;
          if (d2 < 1) { d2 = 1; dx = 1; dy = 0; }
          const d = Math.sqrt(d2);
          const f = (1800 * alpha) / d2;
          const fx = (dx / d) * f;
          const fy = (dy / d) * f;
          nodes[i].vx -= fx; nodes[i].vy -= fy;
          nodes[j].vx += fx; nodes[j].vy += fy;
        }
      }

      // ── Edge attraction
      for (const e of edges) {
        const dx = e.targetNode.x - e.sourceNode.x;
        const dy = e.targetNode.y - e.sourceNode.y;
        const d  = Math.max(1, Math.sqrt(dx * dx + dy * dy));
        const rest = 120 + (1 - e.weight) * 80;
        const f = (d - rest) * 0.003 * alpha * e.weight;
        const fx = (dx / d) * f;
        const fy = (dy / d) * f;
        e.sourceNode.vx += fx; e.sourceNode.vy += fy;
        e.targetNode.vx -= fx; e.targetNode.vy -= fy;
      }

      // ── Center gravity
      const cx = this.W / 2, cy = this.H / 2;
      for (const n of nodes) {
        n.vx += (cx - n.x) * 0.0004 * alpha;
        n.vy += (cy - n.y) * 0.0004 * alpha;
      }

      // ── Apply velocity (semi-implicit Euler) with damping
      for (const n of nodes) {
        n.vx *= 0.88;
        n.vy *= 0.88;
        n.x += n.vx * dt;
        n.y += n.vy * dt;
        // Soft boundary
        const pad = n.radius + 10;
        if (n.x < pad) { n.x = pad; n.vx *= -0.2; }
        if (n.x > this.W - pad) { n.x = this.W - pad; n.vx *= -0.2; }
        if (n.y < pad) { n.y = pad; n.vy *= -0.2; }
        if (n.y > this.H - pad) { n.y = this.H - pad; n.vy *= -0.2; }
      }

      // ── Alpha cooling
      this.simAlpha *= 0.993;
    }

    // ── Smooth viewport interpolation
    this.view.x     += (this.view.targetX - this.view.x) * 0.12;
    this.view.y     += (this.view.targetY - this.view.y) * 0.12;
    this.view.scale += (this.view.targetScale - this.view.scale) * 0.12;

    // ── Focus pulse
    if (this.focusNode) {
      this.focusPulse = (this.focusPulse + 0.03) % 1;
    }

    // ── Fade node opacities
    for (const n of nodes) {
      if (n._fadeDelay > 0) { n._fadeDelay -= 0.016; continue; }
      n.opacity += (n.targetOpacity - n.opacity) * 0.1;
    }

    // ── Advance edge particles
    for (const p of this.edgeParticles) {
      p.t += p.speed;
      if (p.t > 1) p.t -= 1;
    }

    this._render();
  }

  // ── Rendering ───────────────────────────────────────────────────────────

  _render() {
    const ctx = this.ctx;
    const W = this.W, H = this.H;
    const vx = this.view.x, vy = this.view.y, vs = this.view.scale;

    // Clear with subtle radial vignette
    ctx.clearRect(0, 0, W, H);
    const bgGrad = ctx.createRadialGradient(W / 2, H / 2, 0, W / 2, H / 2, Math.max(W, H) * 0.7);
    bgGrad.addColorStop(0, '#111820');
    bgGrad.addColorStop(1, '#080c12');
    ctx.fillStyle = bgGrad;
    ctx.fillRect(0, 0, W, H);

    ctx.save();
    ctx.translate(vx, vy);
    ctx.scale(vs, vs);

    // ── Grid lines (subtle) ───────────────────────────────────────────────
    const gridSize = 60;
    ctx.strokeStyle = 'rgba(148,163,184,0.04)';
    ctx.lineWidth = 1;
    const gx0 = -vx / vs - gridSize;
    const gy0 = -vy / vs - gridSize;
    const gx1 = (W - vx) / vs + gridSize;
    const gy1 = (H - vy) / vs + gridSize;
    ctx.beginPath();
    for (let x = Math.floor(gx0 / gridSize) * gridSize; x < gx1; x += gridSize) {
      ctx.moveTo(x, gy0); ctx.lineTo(x, gy1);
    }
    for (let y = Math.floor(gy0 / gridSize) * gridSize; y < gy1; y += gridSize) {
      ctx.moveTo(gx0, y); ctx.lineTo(gx1, y);
    }
    ctx.stroke();

    // ── Edges ─────────────────────────────────────────────────────────────
    for (const e of this.edges) {
      const style = EDGE_STYLES[e.type] || EDGE_STYLES.associative;
      const sx = e.sourceNode.x, sy = e.sourceNode.y;
      const tx = e.targetNode.x, ty = e.targetNode.y;

      ctx.save();
      ctx.strokeStyle = style.color;
      ctx.lineWidth   = 0.6 + e.weight * 1.5;
      ctx.globalAlpha = 0.3;
      if (style.dash && style.dash.length) ctx.setLineDash(style.dash);
      ctx.beginPath();
      ctx.moveTo(sx, sy);
      ctx.lineTo(tx, ty);
      ctx.stroke();
      ctx.restore();
    }

    // ── Edge particles ────────────────────────────────────────────────────
    if (!this.particleGrad) {
      this.particleGrad = ctx.createRadialGradient(0, 0, 0, 0, 0, 3);
      this.particleGrad.addColorStop(0, 'rgba(255,255,255,0.9)');
      this.particleGrad.addColorStop(0.5, 'rgba(255,255,255,0.4)');
      this.particleGrad.addColorStop(1, 'rgba(255,255,255,0)');
    }
    ctx.fillStyle = this.particleGrad;
    for (const p of this.edgeParticles) {
      const e = p.edge;
      const sx = e.sourceNode.x, sy = e.sourceNode.y;
      const tx = e.targetNode.x, ty = e.targetNode.y;
      const px = sx + (tx - sx) * p.t;
      const py = sy + (ty - sy) * p.t;
      ctx.save();
      ctx.translate(px, py);
      ctx.scale(0.7, 0.7);
      ctx.beginPath();
      ctx.arc(0, 0, 3.5, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    }

    // ── Nodes ─────────────────────────────────────────────────────────────
    for (const n of this.nodes) {
      if (n.opacity < 0.01) continue;
      const colors = LAYER_COLORS[n.layer] || LAYER_COLORS.episodic;
      const isFocused = n === this.focusNode;
      const isHovered = n === this.hoverNode;
      const r  = n.radius;

      ctx.save();
      ctx.globalAlpha = n.opacity;

      // Outer glow ring
      if (isFocused || isHovered) {
        const glowR = r * 3.5 + (isFocused ? Math.sin(this.focusPulse * Math.PI * 2) * 5 : 0);
        const glow = ctx.createRadialGradient(n.x, n.y, r * 0.5, n.x, n.y, glowR);
        glow.addColorStop(0, colors.glow + '80');
        glow.addColorStop(0.5, colors.glow + '20');
        glow.addColorStop(1, 'transparent');
        ctx.fillStyle = glow;
        ctx.beginPath();
        ctx.arc(n.x, n.y, glowR, 0, Math.PI * 2);
        ctx.fill();
      }

      // Focus ring (pulsing)
      if (isFocused) {
        const ringR = r * 2.2 + Math.sin(this.focusPulse * Math.PI * 2) * 3;
        ctx.strokeStyle = colors.ring + '99';
        ctx.lineWidth = 2;
        ctx.setLineDash([4, 4]);
        ctx.lineDashOffset = -this.focusPulse * 20;
        ctx.beginPath();
        ctx.arc(n.x, n.y, ringR, 0, Math.PI * 2);
        ctx.stroke();
        ctx.setLineDash([]);
      }

      // Main node body (radial gradient)
      const bodyGrad = ctx.createRadialGradient(n.x - r * 0.2, n.y - r * 0.2, r * 0.1, n.x, n.y, r);
      bodyGrad.addColorStop(0, '#ffffff');
      bodyGrad.addColorStop(0.3, colors.fill);
      bodyGrad.addColorStop(1, colors.fill + '30');
      ctx.fillStyle = bodyGrad;
      ctx.beginPath();
      ctx.arc(n.x, n.y, r, 0, Math.PI * 2);
      ctx.fill();

      // Outer ring
      ctx.strokeStyle = colors.ring + '60';
      ctx.lineWidth = 0.8;
      ctx.beginPath();
      ctx.arc(n.x, n.y, r + 1, 0, Math.PI * 2);
      ctx.stroke();

      // Label
      const labelSize = Math.max(8, Math.min(11, 12 / vs));
      ctx.font = `${labelSize}px "JetBrains Mono", "Fira Code", ui-monospace, monospace`;
      ctx.fillStyle = '#bcc8d8';
      ctx.globalAlpha = n.opacity * 0.75;
      const label = n.label.slice(0, 35);
      ctx.fillText(label, n.x + r + 5, n.y + labelSize * 0.35);

      // Expanded indicator
      if (n.expanded) {
        ctx.fillStyle = colors.ring;
        ctx.globalAlpha = n.opacity * 0.5;
        ctx.beginPath();
        ctx.arc(n.x + r + 2, n.y - r - 2, 2.5, 0, Math.PI * 2);
        ctx.fill();
      }

      ctx.restore();
    }

    ctx.restore();
  }

  // ── Interaction ─────────────────────────────────────────────────────────

  _screenToWorld(sx, sy) {
    return {
      x: (sx - this.view.x) / this.view.scale,
      y: (sy - this.view.y) / this.view.scale,
    };
  }

  _findNodeAt(wx, wy) {
    // Reverse iterate so top-rendered nodes are picked first
    for (let i = this.nodes.length - 1; i >= 0; i--) {
      const n = this.nodes[i];
      const dx = wx - n.x, dy = wy - n.y;
      if (Math.sqrt(dx * dx + dy * dy) < n.radius + 8) return n;
    }
    return null;
  }

  _onMouseDown(e) {
    const rect = this.canvas.getBoundingClientRect();
    const sx = e.clientX - rect.left, sy = e.clientY - rect.top;
    const w  = this._screenToWorld(sx, sy);
    const node = this._findNodeAt(w.x, w.y);

    this._hasDragged = false;

    if (node) {
      this.dragging = 'node';
      this.dragNode = node;
      this.canvas.style.cursor = 'grabbing';
      node._pinned = true;
    } else {
      this.dragging = 'pan';
      this.dragStart = { x: e.clientX, y: e.clientY, vx: this.view.x, vy: this.view.y };
      this.canvas.style.cursor = 'grabbing';
    }
  }

  _onMouseMove(e) {
    const rect = this.canvas.getBoundingClientRect();
    const sx = e.clientX - rect.left, sy = e.clientY - rect.top;
    const w  = this._screenToWorld(sx, sy);

    if (this.dragging === 'pan') {
      this._hasDragged = true;
      this.view.targetX = this.dragStart.vx + (e.clientX - this.dragStart.x);
      this.view.targetY = this.dragStart.vy + (e.clientY - this.dragStart.y);
      this.view.x = this.view.targetX; this.view.y = this.view.targetY;
    } else if (this.dragging === 'node' && this.dragNode) {
      this._hasDragged = true;
      if (this.tooltip) this.tooltip.style.opacity = '0';
      this.dragNode.x = w.x;
      this.dragNode.y = w.y;
      this.dragNode.vx = 0; this.dragNode.vy = 0;
    } else {
      const node = this._findNodeAt(w.x, w.y);
      if (node !== this.hoverNode) {
        this.hoverNode = node;
        this.canvas.style.cursor = node ? 'pointer' : 'grab';
        if (node) this._showTooltip(e.clientX, e.clientY, node);
        else this._removeTooltip();
      }
      if (this.hoverNode && this.tooltip) {
        this.tooltip.style.left = (e.clientX + 18) + 'px';
        this.tooltip.style.top  = (e.clientY - 10) + 'px';
      }
    }
  }

  _onMouseUp(e) {
    if (this.dragging === 'node' && this.dragNode) {
      this.dragNode._pinned = false;
      if (!this._hasDragged) {
        this._onNodeClick(this.dragNode);
      }
    }
    this.dragging = null;
    this.dragNode = null;
    this.canvas.style.cursor = this.hoverNode ? 'pointer' : 'grab';
  }

  _onWheel(e) {
    const rect = this.canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left, my = e.clientY - rect.top;
    const zoom = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    const newScale = Math.max(0.15, Math.min(4, this.view.targetScale * zoom));

    // Zoom toward cursor
    const wx = (mx - this.view.x) / this.view.scale;
    const wy = (my - this.view.y) / this.view.scale;
    this.view.targetX = mx - wx * newScale;
    this.view.targetY = my - wy * newScale;
    this.view.targetScale = newScale;
  }

  _onNodeClick(node) {
    if (node.expanded) return;
    // Dispatch event so the route handler can fetch related memories
    this.canvas.dispatchEvent(new CustomEvent('mg-node-expand', {
      detail: { id: node.id, label: node.label },
      bubbles: true,
    }));
  }

  _focusNode(id) {
    const node = this.nodeMap.get(id);
    if (!node) return;
    this.focusNode = node;
    this.focusPulse = 0;
    // Center view on node
    const cx = this.W / 2, cy = this.H / 2;
    this.view.targetX = cx - node.x * this.view.scale;
    this.view.targetY = cy - node.y * this.view.scale;
  }

  _resetView() {
    this.view.targetX     = this.W / 2;
    this.view.targetY     = this.H / 2;
    this.view.targetScale = 1;
    this.focusNode        = null;
    this.focusPulse       = 0;
  }

  // ── Tooltip ─────────────────────────────────────────────────────────────

  _showTooltip(cx, cy, node) {
    this._removeTooltip();
    const tip = document.createElement('div');
    tip.className = 'mg-tooltip';
    const colors = LAYER_COLORS[node.layer] || LAYER_COLORS.episodic;
    tip.innerHTML = `
      <div class="mg-tt-header">
        <span class="mg-tt-dot" style="background:${colors.fill}"></span>
        <span class="mg-tt-layer">${node.layer}</span>
        <span class="mg-tt-strength">str ${(node.strength || 0).toFixed(2)}</span>
      </div>
      <div class="mg-tt-content">${esc(node.label)}</div>
      <div class="mg-tt-hint">Click to expand ▸</div>
    `;
    tip.style.left = (cx + 18) + 'px';
    tip.style.top  = (cy - 10) + 'px';
    document.body.appendChild(tip);
    this.tooltip = tip;
  }

  _removeTooltip() {
    if (this.tooltip) { this.tooltip.remove(); this.tooltip = null; }
  }
}

// Re-use the escape helper from main.js (must be globally available)
function esc(s) {
  if (typeof s !== 'string') return '';
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
