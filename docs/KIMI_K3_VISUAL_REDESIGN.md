# Kimi K3 — Engram Memory Vault Visual Redesign Prompt

You are redesigning the Engram Memory Vault UI. The current implementation is
functional but visually bare — 918 lines of minimal CSS, a vanilla JS SPA
(860 lines), and a 45-line HTML shell. Your job is to make it look like a
polished, professional developer tool. Do NOT change any JavaScript logic
(routing, API client, data fetching, event handling). You may only edit:

  styles.css   — full redesign (colors, layout, typography, animations,
                  responsive breakpoints, empty states, component styling)
  index.html   — minor additions only (e.g. an icon font link, meta tags,
                 structural wrappers you need for layout). Do NOT break the
                 existing #app, #main-nav, #statusbar, #toast-root, #modal-root
                 IDs — the JS router depends on them.

The JS code lives in js/main.js (read it for context — DOM structure each
screen renders, CSS classes it references). The API base is now '' (relative
to origin).

---

## What to design

### 1. Design System

Start from these tokens (keep them):

  --bg: #0b0f14;  --bg-raised: #11161d;  --bg-panel: #151b24;
  --episodic: #f59e0b;  --semantic: #3b82f6;  --imagined: #8b5cf6;
  --val-joyful: #10b981;  --val-positive: #14b8a6;
  --val-neutral: #64748b;  --val-challenging: #f59e0b;
  --grounded: #10b981;  --quarantined: #8b5cf6;  --decaying: #94a3b8;
  --danger: #ef4444;  --ok: #10b981;  --accent: #3b82f6;

But ADD:
- A proper typography scale (--text-xs through --text-3xl)
- Spacing scale (--space-1 through --space-8, 4px base)
- Shadow scale (--shadow-sm, --shadow-md, --shadow-lg)
- Transition tokens (--t-fast: 100ms, --t-normal: 200ms, --t-slow: 400ms)
- Border radius scale
- A light theme via [data-theme="light"] on <html> — toggleable, persisted
  to localStorage. Light theme should feel like a bright developer tool
  (cool grays, not warm paper).

### 2. Layout

- Topbar: fixed, 52px tall, glassmorphism blur (backdrop-filter), subtle
  bottom border. Brand gem (◆) should pulse gently on hover.
- Nav: horizontal pills under the brand, active state with a glowing
  underline in the layer color that matches the current section.
  Dashboard=amber, Explorer=blue, Graph=violet, Context=amber, Consolidation=blue.
- Main content area: max-width 1200px centered, padded, scrollable.
- Statusbar: fixed bottom, 28px, muted text, shows vault name + health dot
  (green pulsing = connected) + memory count + uptime. Subtle top border.
- Responsive: below 768px, nav collapses to a hamburger or horizontal
  scroll, cards go single-column, tables become cards, graph becomes a
  simplified list.

### 3. Dashboard Screen (#/ — route '/')

Current JS renders: stats cards in `.card-grid.three`, layer breakdown
in `.layer-grid`, health in `.health-panel`, recent captures as a list.

Enhance:
- Stats cards: each card gets a subtle gradient top-border (3px) in the
  relevant color. Numbers use tabular-nums so they don't jitter on update.
  Add a subtle hover lift (translateY(-2px) + shadow increase).
- Layer breakdown: three cards side-by-side, each with a large layer icon
  (● ◆ ✦) behind the count number at 15% opacity, colored fill bar showing
  percentage of total.
- Health panel: compact row of indicator dots (encrypted=lock icon green,
  QEM warm=zap icon amber, size=file icon muted).
- Recent captures: timeline style — thin colored left border (layer color),
  content preview with elided text, relative timestamp right-aligned, source
  icon, tag chips.
- Loading state: skeleton cards (shimmer animation, 3-4 placeholder blocks).
- Empty state: "No memories captured yet. Connect an ELLM agent to begin."
  with a ◆ icon at 20% opacity centered.

### 4. Explorer Screen (#/memories — route '/memories')

Current JS renders: search input, filter bar, sort dropdown, memory cards.

Enhance:
- Search bar: full-width with a magnifying glass icon (use unicode 🔍),
  subtle glow on focus, clear button appears when text entered.
- Filter pills: horizontal row, each pill is a small chip with the layer
  color as left-dot. Active pill gets filled background in that color at
  15% opacity. Animate the switch with scale.
- Memory cards: card with left accent border (4px, layer color). Top row:
  layer icon + scope badge + timestamp right-aligned. Content preview
  (truncated to 3 lines with fade-out gradient at bottom). Bottom row:
  strength bar (thin, rounded, colored green→amber→red based on value),
  tag chips (small, clickable-looking), link count badge, valence emoji.
  Hover: slight lift + border highlight. Click: subtle ripple.
- Sort dropdown: styled select, custom chevron.
- Keyboard hint: subtle "j/k to navigate · Enter to open" in bottom-right.
- Loading: skeleton card list (3 cards shimmering).
- Empty search results: "No memories match your filters" with illustration
  (◆ at low opacity) and a "Clear filters" button.
- Responsive: cards go full-width, filters wrap to two rows, search bar
  takes less height.

### 5. Detail Screen (#/memories/:id — route '/memories/:id')

Current JS renders: full content card, metadata grid, links list, actions.

Enhance:
- Header: large layer icon + title (first line of content or auto-title).
- Content body: slightly larger text (15px), comfortable line-height (1.7),
  max-width ~720px for readability. Long content gets a subtle "expand"
  fade if it exceeds 8 lines.
- Metadata: definition-list style — label in muted small-caps, value in
  mono. Two-column grid on desktop, single column on mobile.
- Strength bar: large, with numeric percentage label. Animate fill on load.
- Links panel: each link is a mini-card showing link type icon (→ =
  causal, ···· = associative, ---> = analogical, ··→ = temporal), target
  memory preview (first 60 chars), clickable → navigates.
- Action buttons: Ground (green, only for imagined), Delete (red, with
  confirmation modal — do NOT change the JS confirmation logic, just
  style the modal the JS creates at #modal-root).
- Back button: styled ← link at top.

### 6. Graph Screen (#/graph — route '/graph')

Current JS renders: filters row + inline SVG force-directed graph (50
iteration simple layout). This needs the most love.

Enhance (CSS only — don't rewrite the JS graph algorithm, but you CAN
improve how SVG elements are styled):
- Graph container: full height (calc(100vh - 140px)), dark panel background,
  subtle dot-grid pattern (CSS background with radial-gradient).
- SVG nodes: stroke-width 2px, stroke matches layer color at 60% opacity,
  fill is layer color at 25% opacity. On hover: stroke-width 3px,
  opacity 100%, slight scale.
- SVG edges: stroke-dasharray for associative (4,4), solid for causal
  (use stroke color matching the source node's layer). Opacity 30%.
- Filter bar: same pill style as Explorer.
- Legend: positioned bottom-left, semi-transparent panel.
- Empty state: "No links between memories yet. Capture more memories to
  build the graph." Center illustration.
- Loading: pulsing placeholder where graph will be.

### 7. Context Assembly Screen (#/context — route '/context')

Current JS renders: sliders, priority table, toggles, query input + assemble
button, results panel.

Enhance:
- Token budget slider: custom range input styling — track fills with gradient
  (blue→amber→red as it increases), thumb is a small circle with number
  inside showing current value. (Style ::-webkit-slider-thumb and
  ::-moz-range-thumb.)
- Priority table: rows with colored left-border (Required=red, High=amber,
  Normal=blue, Low=muted). Dropdown styled consistently.
- Toggles: custom toggle switch (pill-shaped, slides with transition,
  green when on, gray when off). CSS-only if possible.
- Query input + Assemble button: textarea styled like a chat input,
  button has a subtle glow on hover. On "Assemble" click, show a brief
  pulse animation on the results panel.
- Results panel: dark code-block style, messages rendered as distinct
  bubbles (system=gray, user=blue tint, assistant=green tint). Token count
  badge in top-right of each message. Metadata footer showing
  "12 engrams · 3,400 tokens · 145ms" in mono.
- Loading state: the Assemble button shows a spinner, results panel has
  shimmer.

### 8. Consolidation Screen (#/consolidation — route '/consolidation')

Current JS renders: run history list, stats, patterns, action buttons.

Enhance:
- Timeline: vertical line down the left side with dots at each run.
  Each entry shows timestamp, type (decay/consolidation), stats
  (memories affected, promoted, pruned).
- Decay curve: if JS renders bars/sparklines, style them. Otherwise add
  a CSS-only mini bar chart using div heights.
- Patterns list: each pattern is a card with lightbulb icon (💡),
  description in italic, evidence count badge.
- Action buttons: "Run Decay Now" and "Run Consolidation" styled as
  prominent buttons with a play ▶ icon. On click, brief pulse animation.
- Loading: shimmer on the sections that are loading.

### 9. Settings Screen (#/settings — route '/settings')

Current JS renders: vault info, schedule config, import/export.

Enhance:
- Vault info: key-value pairs in a clean table with alternating row
  backgrounds (subtle).
- Schedule config: time inputs styled consistently.
- Export button: download icon ↓, Import button: upload icon ↑. File
  picker styled (the JS creates an <input type="file"> — style it).
- Danger zone: red-bordered panel at bottom with delete/scrub actions,
  clearly separated.

### 10. Shared Components

- Toast notifications (#toast-root): slide in from right, 4px left border
  colored by type (success=green, error=red, info=blue). Auto-dismiss
  after 4s with a shrinking progress bar at bottom. Animate: slide-in-right
  + fade, exit: slide-out-right + fade.
- Modal (#modal-root): centered, backdrop blur + dark overlay, scale-in
  animation on open. Close button top-right. The JS creates confirm dialogs
  here — style .modal-overlay, .modal-box, .modal-title, .modal-body,
  .modal-actions, .modal-btn, .modal-btn-danger.
- API banner (#api-banner): when visible (JS removes .hidden), slides down
  from top, amber warning style, fixed below topbar. Pulse the ⚠ icon.
- Loading skeletons: consistent shimmer animation (@keyframes shimmer:
  translateX(-100%) → translateX(100%) on a pseudo-element gradient).
  Apply via .skeleton class.
- Scrollbar: thin, dark, rounded (::-webkit-scrollbar). Only 6px wide.
- Selection: ::selection background in accent color at 30% opacity.

### 11. Animations & Polish

- Page transitions: when navigating between routes, fade out old content
  (150ms) → fade in new content (150ms). Do this by styling .app with a
  CSS transition on opacity, and have the JS add/remove a .fade-out class
  (or just style the .loading class that's already used).
- Card hover: transform: translateY(-2px) + box-shadow increase, 200ms ease.
- Button press: transform: scale(0.97), 100ms.
- Number changes: no animation needed (avoid layout thrash), just use
  tabular-nums.
- Graph node hover: scale + glow, 200ms.
- Toast enter/exit: @keyframes slideInRight + slideOutRight.

### 12. Responsive Breakpoints

- ≥1024px: full layout as designed.
- 768–1023px: cards 2-column, nav shrinks font-size, graph takes 70% height.
- <768px: single column, nav becomes horizontal scroll with overflow-x:auto
  and hidden scrollbar (or hamburger menu — but do NOT add JS for this,
  use CSS-only approach like a scrollable nav row). Cards full width.
  Graph becomes simplified (or just show a "Graph available on desktop"
  message with the legend). Filters stack vertically.
- <480px: reduce padding, smaller fonts, hide statusbar on scroll.

### 13. Light Theme

Add [data-theme="light"] to <html> and define:
- --bg: #f8fafc; --bg-raised: #ffffff; --bg-panel: #f1f5f9;
- --text: #0f172a; --text-muted: #475569; --text-faint: #94a3b8;
- All layer/valence colors stay the same (they work on light too).
- --border: #e2e8f0; --border-soft: #f1f5f9;
- Adjust shadows to be lighter.
- Add a small sun/moon toggle in the topbar (do NOT add JS — use a CSS
  class the existing settings JS or a <button> can toggle. Add a
  <button id="theme-toggle">☀</button> in index.html if needed, the JS
  will wire it).

### 14. What NOT to do

- Do NOT modify js/main.js routing, API client, data fetching, or event
  handlers
- Do NOT remove any id attributes from index.html
- Do NOT change the API contract or URL structure
- Do NOT add external dependencies (no CDN fonts, no JS libraries). Use
  system fonts and unicode for icons.
- Do NOT change the HTML skeleton structure — add to it minimally
- Do NOT use CSS frameworks — write vanilla CSS

### Files to edit

1. /home/e/engram/ui/styles.css — full rewrite, keep the existing :root
   tokens as a starting point
2. /home/e/engram/ui/index.html — minor additions only

Read /home/e/engram/ui/js/main.js for context on what CSS classes each
view generates. The JS uses these key selectors you should handle:
  .app, .loading, .error-panel, .mini-bar, .mini-bar-fill,
  .badge-episodic, .badge-semantic, .badge-imagined,
  .layer-icon.episodic, .layer-icon.semantic, .layer-icon.imagined,
  .tag, .valence, .card-grid.three, .feature-grid, .layer-grid,
  .health-panel, .statusbar, .toast-root, #modal-root, #api-banner,
  .nav a.active

Output the complete styles.css and any changes to index.html.
