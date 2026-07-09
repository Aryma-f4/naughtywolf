# UI SPA Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert NaughtyWolf to a lightweight SPA with Neo-cyber styling and Cobalt Strike-style chain graph.

**Architecture:** Axum serves a single SPA shell at `/app` after login. Static JS handles hash routing, API calls, rendering, and graph visualization. CSS is rewritten into a full design system. Old server-rendered routes remain as fallback.

**Tech Stack:** Axum (Rust), vanilla HTML/CSS/JS, native SVG for graph.

## Global Constraints

- No Node.js, no npm, no bundler — pure static files in `static/`.
- Hash routing only: `#/dashboard`, `#/graph`, etc.
- All APIs go through existing `/api/*` endpoints.
- All create/kill/admin operations still require server-side RBAC.
- The login page (`static/login.html`) stays server-rendered but must match the new style.
- `cargo check`, `cargo test`, `cargo clippy` must pass.
- SPA must not break when Sliver is disconnected — show empty/error states.
- Emoji nav icons replaced with compact glyph style.

---
### Task 1: SPA shell HTML and Axum route

**Files:**
- Create: `static/app.html`
- Modify: `src/web/routes.rs:30-46`

**Interfaces:**
- Consumes: `AuthenticatedUserGuard`, `AppState`, `templates::PageContext`
- Produces: SPA shell served at `/app` route after authentication

- [ ] **Step 1: Create `static/app.html` SPA shell**

```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>NaughtyWolf</title>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
<div id="app-shell"></div>
<script src="/static/app.js"></script>
</body>
</html>
```

- [ ] **Step 2: Add `/app` route to `src/web/routes.rs`**

Add import + route + handler function:

```rust
// After the .route("/loot", get(loot_page)) line, add:
        .route("/app", get(spa_shell))

// Add handler function at bottom of routes.rs:
async fn spa_shell(user: AuthenticatedUserGuard) -> impl IntoResponse {
    Html(include_str!("../../static/app.html"))
}
```

- [ ] **Step 3: Verify compilation**

```bash
cd /Users/dsi/projects/naughtywolf && cargo check
```
Expected: clean

- [ ] **Step 4: Commit**

```bash
cd /Users/dsi/projects/naughtywolf && git add static/app.html src/web/routes.rs && git status
git commit -m "feat: add SPA shell route and static HTML"
```

---
### Task 2: Neo-cyber CSS design system

**Files:**
- Modify: `static/style.css` (complete rewrite)

**Interfaces:**
- Consumes: CSS variables defined in `:root`
- Produces: All SPA styles based on those variables

- [ ] **Step 1: Rewrite `static/style.css` with full design system**

Replace the entire file:

```css
/* ============================================================
   NaughtyWolf Neo-Cyber Design System
   ============================================================ */

:root {
  --bg-app: #060a12;
  --bg-panel: #0c1422;
  --bg-raised: #111b2e;
  --bg-overlay: rgba(6,10,18,0.85);
  --bg-input: #070b14;

  --border-subtle: #1a2a44;
  --border-active: #38bdf8;
  --border-danger: #fb7185;

  --text: #dce8f5;
  --text-muted: #7895b8;
  --text-dim: #3c5470;

  --cyan: #38bdf8;
  --cyan-glow: rgba(56,189,248,0.25);
  --cyan-soft: #7dd3fc;
  --magenta: #f472b6;
  --magenta-glow: rgba(244,114,182,0.2);
  --green: #34d399;
  --green-glow: rgba(52,211,153,0.2);
  --amber: #fbbf24;
  --red: #fb7185;
  --red-glow: rgba(251,113,133,0.2);

  --radius-sm: 4px;
  --radius-md: 8px;
  --radius-lg: 12px;
  --shadow: 0 2px 12px rgba(0,0,0,0.3);
  --spacing-xs: 4px;
  --spacing-sm: 8px;
  --spacing-md: 16px;
  --spacing-lg: 24px;
  --font-mono: 'SF Mono','Fira Code','Cascadia Code',monospace;
  --font-ui: -apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;
  --transition: 0.15s ease;
}

*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

html, body {
  height: 100%;
  background: var(--bg-app);
  color: var(--text);
  font-family: var(--font-ui);
  font-size: 14px;
  line-height: 1.5;
  -webkit-font-smoothing: antialiased;
}

a { color: var(--cyan); text-decoration: none; transition: color var(--transition); }
a:hover { color: var(--cyan-soft); }
svg { display: block; }

/* ==============================
   APP SHELL
   ============================== */

.app-shell {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

.side-rail {
  width: 220px;
  background: var(--bg-panel);
  border-right: 1px solid var(--border-subtle);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  overflow-y: auto;
}

.brand-block {
  padding: var(--spacing-lg) var(--spacing-md);
  border-bottom: 1px solid var(--border-subtle);
}

.brand-block h1 {
  font-size: 1.15rem;
  font-weight: 700;
  color: var(--cyan);
  letter-spacing: 1px;
  text-transform: uppercase;
}

.brand-block .brand-sub {
  font-size: 0.7rem;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 1.5px;
  margin-top: 2px;
}

.conn-badge {
  display: inline-block;
  margin-top: var(--spacing-sm);
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 0.65rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}
.conn-badge.online { background: rgba(52,211,153,0.15); color: var(--green); }
.conn-badge.offline { background: rgba(251,113,133,0.15); color: var(--red); }

.nav-list {
  list-style: none;
  padding: var(--spacing-sm) 0;
  flex: 1;
}

.nav-link {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px var(--spacing-md);
  color: var(--text-muted);
  font-size: 0.8rem;
  cursor: pointer;
  transition: all var(--transition);
  border-left: 3px solid transparent;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.nav-link:hover {
  background: rgba(56,189,248,0.05);
  color: var(--text);
}

.nav-link.active {
  color: var(--cyan);
  border-left-color: var(--cyan);
  background: rgba(56,189,248,0.08);
}

.nav-link .glyph {
  width: 18px;
  text-align: center;
  font-size: 0.85rem;
  opacity: 0.7;
}

.nav-section-title {
  padding: var(--spacing-md) var(--spacing-md) var(--spacing-xs);
  font-size: 0.6rem;
  text-transform: uppercase;
  letter-spacing: 1.5px;
  color: var(--text-dim);
}

.side-rail-footer {
  padding: var(--spacing-md);
  border-top: 1px solid var(--border-subtle);
  font-size: 0.75rem;
  color: var(--text-dim);
}

/* ==============================
   TOPBAR
   ============================== */

.status-strip {
  height: 44px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border-subtle);
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 var(--spacing-lg);
  flex-shrink: 0;
}

.status-strip-left { display: flex; align-items: center; gap: var(--spacing-md); }
.status-strip-right { display: flex; align-items: center; gap: var(--spacing-md); }

.page-title {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.user-badge {
  color: var(--text-muted);
  font-size: 0.75rem;
}

.logout-link {
  font-size: 0.75rem;
  color: var(--red);
  cursor: pointer;
  transition: color var(--transition);
}
.logout-link:hover { color: var(--magenta); }

/* ==============================
   MAIN CONTENT
   ============================== */

.main-panel {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.content-area {
  flex: 1;
  padding: var(--spacing-lg);
  overflow-y: auto;
}

/* ==============================
   METRIC CARDS
   ============================== */

.metric-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: var(--spacing-md);
  margin-bottom: var(--spacing-lg);
}

.metric-card {
  background: var(--bg-panel);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  padding: var(--spacing-md);
  transition: border-color var(--transition);
}

.metric-card:hover { border-color: var(--border-active); }

.metric-card .metric-label {
  font-size: 0.65rem;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-muted);
  margin-bottom: var(--spacing-xs);
}

.metric-card .metric-value {
  font-size: 1.75rem;
  font-weight: 700;
  font-family: var(--font-mono);
  color: var(--cyan);
}

.metric-card .metric-value.warning { color: var(--amber); }
.metric-card .metric-value.danger { color: var(--red); }

/* ==============================
   PANELS
   ============================== */

.panel {
  background: var(--bg-panel);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  margin-bottom: var(--spacing-md);
}

.panel-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--spacing-sm) var(--spacing-md);
  border-bottom: 1px solid var(--border-subtle);
}

.panel-header h3 {
  font-size: 0.7rem;
  text-transform: uppercase;
  letter-spacing: 1px;
  color: var(--text-muted);
}

.panel-body {
  padding: var(--spacing-md);
}

/* ==============================
   DATA TABLE
   ============================== */

.data-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.8rem;
}

.data-table thead th {
  background: var(--bg-raised);
  padding: 8px var(--spacing-sm);
  text-align: left;
  font-size: 0.65rem;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-muted);
  border-bottom: 1px solid var(--border-subtle);
  font-weight: 600;
}

.data-table tbody td {
  padding: 8px var(--spacing-sm);
  border-bottom: 1px solid var(--border-subtle);
  color: var(--text);
  font-size: 0.8rem;
}

.data-table tbody tr:hover {
  background: rgba(56,189,248,0.03);
}

.data-table tbody tr:last-child td {
  border-bottom: none;
}

/* ==============================
   BADGES
   ============================== */

.badge {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 0.65rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.3px;
}

.badge-active { background: rgba(52,211,153,0.15); color: var(--green); }
.badge-dead { background: rgba(251,113,133,0.15); color: var(--red); }
.badge-warning { background: rgba(251,191,36,0.15); color: var(--amber); }
.badge-unknown { background: rgba(120,149,184,0.15); color: var(--text-muted); }

/* ==============================
   BUTTONS
   ============================== */

.btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 14px;
  border-radius: var(--radius-sm);
  font-size: 0.75rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition);
  border: 1px solid transparent;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  font-family: var(--font-ui);
  line-height: 1;
}

.btn-primary {
  background: var(--cyan);
  color: var(--bg-app);
  border-color: var(--cyan);
}
.btn-primary:hover {
  background: var(--cyan-soft);
  box-shadow: 0 0 16px var(--cyan-glow);
}
.btn-primary:disabled {
  opacity: 0.4;
  cursor: not-allowed;
  box-shadow: none;
}

.btn-danger {
  background: transparent;
  color: var(--red);
  border-color: var(--red);
}
.btn-danger:hover {
  background: var(--red-glow);
}

.btn-ghost {
  background: transparent;
  color: var(--text-muted);
  border-color: var(--border-subtle);
}
.btn-ghost:hover {
  color: var(--text);
  border-color: var(--text-muted);
}

.btn-sm { padding: 4px 10px; font-size: 0.7rem; }

/* ==============================
   FORMS
   ============================== */

.form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--spacing-sm) var(--spacing-md);
}

.field { display: flex; flex-direction: column; gap: 4px; }
.field.full { grid-column: span 2; }

.field label {
  font-size: 0.7rem;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-muted);
}

.input, .select {
  padding: 7px 10px;
  background: var(--bg-input);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  color: var(--text);
  font-size: 0.8rem;
  font-family: var(--font-mono);
  outline: none;
  transition: border-color var(--transition);
}

.input:focus, .select:focus {
  border-color: var(--cyan);
  box-shadow: 0 0 8px var(--cyan-glow);
}

.select {
  appearance: none;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='%237895b8'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 10px center;
  padding-right: 28px;
}

/* ==============================
   STATUS / NOTICE
   ============================== */

.notice {
  padding: var(--spacing-sm) var(--spacing-md);
  border-radius: var(--radius-sm);
  font-size: 0.8rem;
  margin-bottom: var(--spacing-sm);
}

.notice-info { background: rgba(56,189,248,0.1); border: 1px solid var(--cyan); color: var(--cyan-soft); }
.notice-error { background: rgba(251,113,133,0.1); border: 1px solid var(--red); color: var(--red); }
.notice-success { background: rgba(52,211,153,0.1); border: 1px solid var(--green); color: var(--green); }

/* ==============================
   EMPTY STATE
   ============================== */

.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 48px var(--spacing-md);
  text-align: center;
}

.empty-state .empty-icon {
  font-size: 2rem;
  color: var(--text-dim);
  margin-bottom: var(--spacing-sm);
}

.empty-state h3 {
  font-size: 0.85rem;
  color: var(--text-muted);
  margin-bottom: var(--spacing-xs);
}

.empty-state p {
  font-size: 0.8rem;
  color: var(--text-dim);
}

/* ==============================
   LOADING SPINNER
   ============================== */

.loading {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 48px var(--spacing-md);
}

.spinner {
  width: 24px;
  height: 24px;
  border: 2px solid var(--border-subtle);
  border-top-color: var(--cyan);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin { to { transform: rotate(360deg); } }

/* ==============================
   TERMINAL PANEL
   ============================== */

.terminal-panel {
  background: var(--bg-app);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  padding: var(--spacing-sm);
  font-family: var(--font-mono);
  font-size: 0.75rem;
  line-height: 1.6;
  max-height: 400px;
  overflow-y: auto;
}

.terminal-line {
  padding: 1px 4px;
  color: var(--text-muted);
  white-space: pre;
}

.terminal-line .ts { color: var(--text-dim); }
.terminal-line .evt { color: var(--cyan); }

/* ==============================
   CHAIN GRAPH
   ============================== */

.graph-container {
  background: var(--bg-app);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  overflow: hidden;
  position: relative;
  min-height: 500px;
}

.graph-container svg {
  display: block;
  width: 100%;
  height: 100%;
  min-height: 500px;
}

.graph-node rect, .graph-node circle {
  transition: stroke-opacity var(--transition), filter var(--transition);
}

.graph-node:hover rect, .graph-node:hover circle {
  stroke-opacity: 1;
  filter: brightness(1.3);
}

.graph-edge {
  stroke: var(--border-subtle);
  stroke-width: 2;
  fill: none;
  transition: stroke var(--transition);
}

.graph-edge.active {
  stroke: var(--cyan);
  stroke-width: 2;
  stroke-dasharray: 6 3;
  animation: dash-flow 1s linear infinite;
}

@keyframes dash-flow {
  to { stroke-dashoffset: -9; }
}

.graph-node-label {
  font-family: var(--font-mono);
  font-size: 10px;
  fill: var(--text-muted);
}

.graph-node-title {
  font-family: var(--font-ui);
  font-size: 10px;
  font-weight: 600;
  fill: var(--text);
}

.graph-details {
  position: absolute;
  right: var(--spacing-md);
  top: var(--spacing-md);
  background: var(--bg-panel);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  padding: var(--spacing-sm) var(--spacing-md);
  min-width: 200px;
  font-size: 0.75rem;
}

.graph-details h4 {
  color: var(--cyan);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  font-size: 0.65rem;
  margin-bottom: var(--spacing-xs);
}

.graph-details .detail-row {
  display: flex;
  justify-content: space-between;
  padding: 2px 0;
  color: var(--text-muted);
}

.graph-toolbar {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
  margin-bottom: var(--spacing-sm);
}

/* ==============================
   LOGIN PAGE (restyled)
   ============================== */

.login-page {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 100vh;
  background: var(--bg-app);
  padding: var(--spacing-lg);
  background-image:
    radial-gradient(ellipse at 50% 0%, rgba(56,189,248,0.08) 0%, transparent 60%),
    radial-gradient(ellipse at 50% 100%, rgba(244,114,182,0.04) 0%, transparent 60%);
}

.login-card {
  background: var(--bg-panel);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  padding: 48px 40px 40px;
  width: 100%;
  max-width: 380px;
  box-shadow: 0 4px 32px rgba(0,0,0,0.4);
}

.login-card h1 {
  text-align: center;
  font-size: 1.5rem;
  color: var(--cyan);
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 2px;
  margin-bottom: 4px;
}

.login-card .subtitle {
  text-align: center;
  color: var(--text-muted);
  font-size: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 2px;
  margin-bottom: 32px;
}

.error-message {
  background: var(--red-glow);
  border: 1px solid var(--red);
  color: var(--red);
  padding: var(--spacing-sm) var(--spacing-md);
  border-radius: var(--radius-sm);
  margin-bottom: var(--spacing-md);
  font-size: 0.8rem;
}

.form-group {
  margin-bottom: var(--spacing-md);
}

.form-group label {
  display: block;
  margin-bottom: 6px;
  color: var(--text-muted);
  font-size: 0.7rem;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.form-group input {
  width: 100%;
  padding: 10px 12px;
  background: var(--bg-input);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  color: var(--text);
  font-size: 0.85rem;
  outline: none;
  font-family: var(--font-mono);
  transition: border-color var(--transition);
}

.form-group input:focus {
  border-color: var(--cyan);
  box-shadow: 0 0 8px var(--cyan-glow);
}

.login-card .btn-primary {
  margin-top: var(--spacing-sm);
}

/* ==============================
   SCROLLBAR
   ============================== */

::-webkit-scrollbar { width: 6px; height: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border-subtle); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: var(--text-dim); }

/* ==============================
   RESPONSIVE
   ============================== */

@media (max-width: 768px) {
  .side-rail { width: 52px; }
  .side-rail .brand-block h1 { font-size: 0.9rem; }
  .side-rail .brand-block .brand-sub { display: none; }
  .side-rail .nav-link span { display: none; }
  .side-rail .nav-link { justify-content: center; padding: 10px 0; }
  .form-grid { grid-template-columns: 1fr; }
  .metric-grid { grid-template-columns: 1fr 1fr; }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cd /Users/dsi/projects/naughtywolf && cargo check
```
Expected: clean (CSS is a static file, no Rust compile needed for this task, but verify the server still compiles)

- [ ] **Step 3: Commit**

```bash
cd /Users/dsi/projects/naughtywolf && git add static/style.css && git commit -m "feat: complete Neo-cyber CSS design system"
```

---
### Task 3: SPA core JavaScript — router, API, shared components

**Files:**
- Create: `static/app.js`

**Interfaces:**
- Produces: `window.router`, `apiGet()`, `apiPost()`, `renderLoading()`, `renderEmpty()`, `renderError()`, `showNotice()`, nav helpers, shared table rendering, page registry

- [ ] **Step 1: Create `static/app.js` with router core and API layer**

All code goes in one file (no bundler). Create file with:

```javascript
// ── Router ──────────────────────────────────────────────────
(function() {
  'use strict';

  const routes = {};
  let currentCleanup = null;

  function register(path, renderFn) {
    routes[path] = renderFn;
  }

  function navigate(hash) {
    const path = hash.replace(/^#/, '') || '/dashboard';
    const renderFn = routes[path];
    const main = document.getElementById('main-content');
    if (!main) return;

    // Cleanup previous page
    if (currentCleanup && typeof currentCleanup === 'function') {
      try { currentCleanup(); } catch(e) { console.warn('cleanup error', e); }
      currentCleanup = null;
    }

    // Update nav active state
    document.querySelectorAll('.nav-link').forEach(el => {
      el.classList.toggle('active', el.dataset.route === path);
    });

    // Update page title
    const titleEl = document.getElementById('page-title');
    if (titleEl && renderFn) {
      const label = document.querySelector(`.nav-link[data-route="${path}"]`)?.querySelector('span')?.textContent || 'Dashboard';
      titleEl.textContent = label;
    }

    if (!renderFn) {
      main.innerHTML = '<div class="empty-state"><div class="empty-icon">⚠</div><h3>Page not found</h3></div>';
      return;
    }

    currentCleanup = renderFn(main);
  }

  window.addEventListener('hashchange', () => navigate(window.location.hash));
  window.addEventListener('DOMContentLoaded', () => navigate(window.location.hash || '#/dashboard'));

  window.router = { register, navigate: (hash) => { window.location.hash = hash; } };

  // ── API Layer ──────────────────────────────────────────────

  window.apiGet = async function(path) {
    const res = await fetch(path, { credentials: 'same-origin' });
    if (res.redirected || res.status === 401) {
      window.location.href = '/login';
      throw new Error('Unauthorized');
    }
    if (!res.ok) {
      const text = await res.text().catch(() => 'Unknown error');
      throw new Error(`API ${res.status}: ${text.slice(0, 200)}`);
    }
    return res.json();
  };

  window.apiPost = async function(path, body) {
    const res = await fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      credentials: 'same-origin',
    });
    if (res.redirected || res.status === 401) {
      window.location.href = '/login';
      throw new Error('Unauthorized');
    }
    if (!res.ok) {
      const text = await res.text().catch(() => 'Unknown error');
      throw new Error(`API ${res.status}: ${text.slice(0, 200)}`);
    }
    return res.json();
  };

  window.apiDelete = async function(path) {
    const res = await fetch(path, {
      method: 'DELETE',
      credentials: 'same-origin',
    });
    if (res.redirected || res.status === 401) {
      window.location.href = '/login';
      throw new Error('Unauthorized');
    }
    if (!res.ok) {
      const text = await res.text().catch(() => 'Unknown error');
      throw new Error(`API ${res.status}: ${text.slice(0, 200)}`);
    }
    return res.json().catch(() => ({}));
  };

  // ── Shared Render Helpers ──────────────────────────────────

  window.renderLoading = function() {
    return '<div class="loading"><div class="spinner"></div></div>';
  };

  window.renderError = function(msg) {
    return `<div class="empty-state"><div class="empty-icon">⚠</div><h3>Error</h3><p>${escapeHtml(msg)}</p></div>`;
  };

  window.renderEmpty = function(icon, title, detail) {
    return `<div class="empty-state"><div class="empty-icon">${icon}</div><h3>${escapeHtml(title)}</h3><p>${escapeHtml(detail)}</p></div>`;
  };

  window.renderTable = function(headers, rows) {
    if (!rows || rows.length === 0) return '';
    const thead = headers.map(h => `<th>${escapeHtml(h)}</th>`).join('');
    const tbody = rows.map(row => `<tr>${row.map(c => `<td>${c}</td>`).join('')}</tr>`).join('');
    return `<table class="data-table"><thead><tr>${thead}</tr></thead><tbody>${tbody}</tbody></table>`;
  };

  window.showNotice = function(el, type, msg) {
    el.innerHTML = `<div class="notice notice-${type}">${escapeHtml(msg)}</div>`;
  };

  function escapeHtml(s) {
    if (s == null) return '';
    return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
  }

  window.escapeHtml = escapeHtml;

  // ── Connection badge helper ────────────────────────────────

  window.updateConnBadge = async function() {
    try {
      const status = await apiGet('/api/sliver/status');
      const badge = document.getElementById('conn-badge');
      if (badge) {
        if (status.connected) {
          badge.className = 'conn-badge online';
          badge.textContent = status.profile_name || 'connected';
        } else {
          badge.className = 'conn-badge offline';
          badge.textContent = 'disconnected';
        }
      }
    } catch { /* server down, leave badge as-is */ }
  };
})();
```

- [ ] **Step 2: Commit**

```bash
cd /Users/dsi/projects/naughtywolf && git add static/app.js && git commit -m "feat: add SPA JavaScript core with router, API layer, and shared components"
```

---
### Task 4: Dashboard SPA page

**Files:**
- Modify: `static/app.js` (register dashboard page)

- [ ] **Step 1: Add dashboard route and renderer to `static/app.js`**

Append before the closing `})()`:

```javascript
(function registerPages() {
  // ── Dashboard ────────────────────────────────────────────
  window.router.register('/dashboard', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;

    (async () => {
      try {
        const stats = await apiGet('/api/dashboard/stats')
          .catch(() => ({ active_listeners: 0, sessions: 0, beacons: 0, jobs: 0 }));

        if (cancelled) return;

        // Metric cards
        const metrics = `
          <div class="metric-grid">
            <div class="metric-card">
              <div class="metric-label">Listeners</div>
              <div class="metric-value">${stats.active_listeners || 0}</div>
            </div>
            <div class="metric-card">
              <div class="metric-label">Sessions</div>
              <div class="metric-value">${stats.sessions || 0}</div>
            </div>
            <div class="metric-card">
              <div class="metric-label">Beacons</div>
              <div class="metric-value">${stats.beacons || 0}</div>
            </div>
            <div class="metric-card">
              <div class="metric-label">Jobs</div>
              <div class="metric-value">${stats.jobs || 0}</div>
            </div>
          </div>`;

        // Graph preview panel
        const graphPreview = `
          <div class="panel">
            <div class="panel-header">
              <h3>Infrastructure Graph</h3>
              <a href="#" onclick="window.router.navigate('#/graph');return false" class="btn btn-ghost btn-sm">View Graph</a>
            </div>
            <div class="panel-body" style="text-align:center;padding:24px;color:var(--text-dim);font-size:0.8rem;">
              View live chain graph showing listeners, sessions, and beacons.
            </div>
          </div>`;

        // Event panel uses SSE in the dedicated Events page. Dashboard shows stable empty state.
        const eventPanel = `
          <div class="panel">
            <div class="panel-header"><h3>Event Feed</h3></div>
            <div class="terminal-panel" style="max-height:250px">
              <div class="terminal-line" style="color:var(--text-dim)">Open Events for live SSE stream.</div>
            </div>
          </div>`;

        main.innerHTML = metrics + graphPreview + eventPanel;
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();

    return () => { cancelled = true; };
  });
})();
```

- [ ] **Step 2: Verify static server serves `/app` with login redirect**

Dev server is already running at `127.0.0.1:8080`. Quick check:
```bash
curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/app
```
Expected: 302 redirect to login (or 200 if already authenticated)

- [ ] **Step 3: Commit**

```bash
cd /Users/dsi/projects/naughtywolf && git add static/app.js && git commit -m "feat: add dashboard SPA page with metrics and event feed"
```

---
### Task 5: Data pages — Sessions, Beacons, Listeners, Payloads, Websites, Loot, Credentials

**Files:**
- Modify: `static/app.js` (register 7 data page routes)

Each page follows the same pattern:
1. Show loading spinner
2. Try API fetch
3. On success, render data table
4. On error, show error if Sliver disconnected vs server error
5. On empty, show empty state
6. Return cleanup function

- [ ] **Step 1: Add sessions page at end of `registerPages()`**

```javascript
  // ── Sessions ──────────────────────────────────────────────
  window.router.register('/sessions', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const sessions = await apiGet('/api/sessions');
        if (cancelled) return;
        if (!sessions || sessions.length === 0) {
          main.innerHTML = window.renderEmpty('💻', 'No Sessions', 'Connect to Sliver and wait for implants to check in.');
          return;
        }
        const rows = sessions.map(s => [
          escapeHtml(s.id),
          escapeHtml(s.name),
          escapeHtml(s.hostname),
          escapeHtml(s.username),
          escapeHtml(s.transport),
          `<span class="badge ${s.status === 'Active' ? 'badge-active' : 'badge-dead'}">${escapeHtml(s.status)}</span>`,
        ]);
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Sessions</h3></div><div class="panel-body">'
          + window.renderTable(['ID','Name','Hostname','User','Transport','Status'], rows)
          + '</div></div>';
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Beacons ──────────────────────────────────────────────
  window.router.register('/beacons', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const beacons = await apiGet('/api/beacons');
        if (cancelled) return;
        if (!beacons || beacons.length === 0) {
          main.innerHTML = window.renderEmpty('📡', 'No Beacons', 'Connect to Sliver and wait for beacons to check in.');
          return;
        }
        const rows = beacons.map(b => [
          escapeHtml(b.name),
          escapeHtml(b.hostname),
          escapeHtml(b.transport),
          escapeHtml(b.last_checkin),
          `<span class="badge ${b.status === 'Active' ? 'badge-active' : 'badge-dead'}">${escapeHtml(b.status)}</span>`,
        ]);
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Beacons</h3></div><div class="panel-body">'
          + window.renderTable(['Name','Hostname','Transport','Last Checkin','Status','ID'], rows)
          + '</div></div>';
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Listeners ────────────────────────────────────────────
  window.router.register('/listeners', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const listeners = await apiGet('/api/listeners');
        if (cancelled) return;
        if (!listeners || listeners.length === 0) {
          main.innerHTML = window.renderEmpty('👂', 'No Listeners', 'Connect to Sliver and start a listener job.');
          return;
        }
        const rows = listeners.map(l => [
          escapeHtml(l.id),
          escapeHtml(l.protocol),
          escapeHtml(l.bind),
          `<span class="badge badge-active">${escapeHtml(l.status)}</span>`,
        ]);
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Active Listeners</h3></div><div class="panel-body">'
          + window.renderTable(['ID','Protocol','Bind','Status'], rows)
          + '</div></div>';
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Payloads ─────────────────────────────────────────────
  window.router.register('/payloads', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const builds = await apiGet('/api/payloads');
        if (cancelled) return;
        const fmtNames = ['executable','shared lib','shellcode','service'];
        const rows = (builds || []).map(b => [
          escapeHtml(b.name),
          `${escapeHtml(b.goos)}/${escapeHtml(b.goarch)}`,
          b.is_beacon ? 'beacon' : 'session',
          fmtNames[b.format] || 'unknown',
        ]);
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Implant Builds</h3></div><div class="panel-body">'
          + (rows.length > 0
              ? window.renderTable(['Name','OS/Arch','Type','Format'], rows)
              : '<div class="empty-state"><div class="empty-icon">📦</div><h3>No Payloads</h3><p>Generate a payload to see it here.</p></div>')
          + '</div></div>';
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Websites ─────────────────────────────────────────────
  window.router.register('/websites', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const sites = await apiGet('/api/websites');
        if (cancelled) return;
        if (!sites || sites.length === 0) {
          main.innerHTML = window.renderEmpty('🌐', 'No Websites', 'Connect to Sliver and configure websites.');
          return;
        }
        const rows = sites.map(s => [
          escapeHtml(s.id || s.name),
          escapeHtml(s.name),
          String(s.content_count || 0),
          String(s.total_size || 0) + ' bytes',
        ]);
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Websites</h3></div><div class="panel-body">'
          + window.renderTable(['ID','Name','Content Items','Total Size'], rows)
          + '</div></div>';
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Loot ─────────────────────────────────────────────────
  window.router.register('/loot', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const items = await apiGet('/api/loot');
        if (cancelled) return;
        if (!items || items.length === 0) {
          main.innerHTML = window.renderEmpty('💰', 'No Loot', 'Connect to Sliver and collect loot from implants.');
          return;
        }
        const rows = items.map(l => [
          escapeHtml(l.id || l.name),
          escapeHtml(l.name),
          escapeHtml(l.file_type || 'unknown'),
          String(l.size || 0) + ' bytes',
        ]);
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Loot</h3></div><div class="panel-body">'
          + window.renderTable(['ID','Name','File Type','Size'], rows)
          + '</div></div>';
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Credentials ──────────────────────────────────────────
  window.router.register('/creds', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const creds = await apiGet('/api/creds');
        if (cancelled) return;
        if (!creds || creds.length === 0) {
          main.innerHTML = window.renderEmpty('🔑', 'No Credentials', 'Connect to Sliver to manage collected credentials.');
          return;
        }
        const rows = creds.map(c => [
          escapeHtml(c.id || c.collection),
          escapeHtml(c.username || '—'),
          escapeHtml(c.hash_type || '—'),
          c.is_cracked ? `<span class="badge badge-active">Yes</span>` : `<span class="badge badge-unknown">No</span>`,
          escapeHtml(c.collection || '—'),
        ]);
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Credentials</h3></div><div class="panel-body">'
          + window.renderTable(['Collection','Username','Hash Type','Cracked','Collection'], rows)
          + '</div></div>';
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });
```

- [ ] **Step 2: Commit**

```bash
cd /Users/dsi/projects/naughtywolf && git add static/app.js && git commit -m "feat: add SPA data pages for sessions, beacons, listeners, payloads, websites, loot, creds"
```

---
### Task 6: Events, Admin, Audit SPA pages

**Files:**
- Modify: `static/app.js` (register 3 more pages)

- [ ] **Step 1: Add events, admin, audit pages**

Add inside `registerPages()` before the final `})();`:

```javascript
  // ── Events ───────────────────────────────────────────────
  window.router.register('/events', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const events = await apiGet('/api/events');
        if (cancelled) return;
        const lines = Array.isArray(events) && events.length > 0
          ? events.map(e => `<div class="terminal-line">${escapeHtml(JSON.stringify(e))}</div>`).join('')
          : '<div class="terminal-line" style="color:var(--text-dim)">No events yet</div>';
        main.innerHTML = `<div class="panel"><div class="panel-header"><h3>Event Stream</h3></div><div class="terminal-panel" style="max-height:600px">${lines}</div></div>`;
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Admin ────────────────────────────────────────────────
  window.router.register('/admin', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const users = await apiGet('/api/users');
        if (cancelled) return;
        if (!users || users.length === 0) {
          main.innerHTML = window.renderEmpty('⚙️', 'No Users', 'Create users from the CLI.');
          return;
        }
        const rows = users.map(u => [
          escapeHtml(u.username),
          `<span class="badge ${u.role === 'admin' ? 'badge-active' : 'badge-unknown'}">${escapeHtml(u.role)}</span>`,
          u.disabled ? `<span class="badge badge-dead">Disabled</span>` : `<span class="badge badge-active">Active</span>`,
          escapeHtml(u.created_at || '—'),
        ]);
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>User Management</h3></div><div class="panel-body">'
          + window.renderTable(['Username','Role','Status','Created'], rows)
          + '</div></div>';
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Audit ────────────────────────────────────────────────
  window.router.register('/audit', function(main) {
    main.innerHTML = window.renderEmpty('📋', 'Audit Log', 'Audit log feature pending implementation.');
  });
```

- [ ] **Step 2: Commit**

```bash
cd /Users/dsi/projects/naughtywolf && git add static/app.js && git commit -m "feat: add events, admin, audit SPA pages"
```

---
### Task 7: Chain Graph visualization

**Files:**
- Modify: `static/app.js` (register `/graph` route with SVG renderer)

- [ ] **Step 1: Add graph page with native SVG renderer**

Add inside `registerPages()`:

```javascript
  // ── Chain Graph ──────────────────────────────────────────
  window.router.register('/graph', function(main) {
    let cancelled = false;
    const renderGraph = async () => {
      main.innerHTML = window.renderLoading();
      try {
        const [status, listeners, sessions, beacons] = await Promise.all([
          apiGet('/api/sliver/status').catch(() => ({connected:false})),
          apiGet('/api/listeners').catch(() => []),
          apiGet('/api/sessions').catch(() => []),
          apiGet('/api/beacons').catch(() => []),
        ]);
        if (cancelled) return;

        const W = 900, H = 500;
        const COL1 = 150, COL2 = 350, COL3 = 600, COL4 = 780;
        const SPACING = 80;

        // Build nodes and edges
        const nodes = [];
        const edges = [];

        // Column 1: Sliver root node
        const rootLabel = status.connected ? (status.profile_name || 'Sliver Server') : 'Sliver (disconnected)';
        nodes.push({ id: 'root', label: rootLabel, x: COL1, y: 250, color: status.connected ? '#38bdf8' : '#7895b8', type: 'server' });

        // Column 2: Listeners
        const listenArr = Array.isArray(listeners) ? listeners : [];
        listenArr.forEach((l, i) => {
          const y = 100 + i * SPACING;
          const nid = `listener-${l.id}`;
          nodes.push({ id: nid, label: `${l.protocol}:${l.port}`, sub: l.bind, x: COL2, y, color: '#34d399', type: 'listener' });
          edges.push({ from: 'root', to: nid, label: 'listener' });
        });
        if (listenArr.length === 0) {
          nodes.push({ id: 'no-listener', label: 'No listeners', x: COL2, y: 250, color: '#7895b8', type: 'empty' });
        }

        // Column 3: Sessions + Beacons
        const sessArr = Array.isArray(sessions) ? sessions : [];
        const beaconArr = Array.isArray(beacons) ? beacons : [];
        const implants = [
          ...sessArr.map(s => ({ ...s, implantType: 'session' })),
          ...beaconArr.map(b => ({ ...b, implantType: 'beacon' })),
        ];
        const col3Count = Math.max(implants.length, 1);
        implants.forEach((im, i) => {
          const y = 60 + i * Math.min(SPACING, 480 / col3Count);
          const nid = `implant-${im.id || i}`;
          const statusColor = (im.status === 'Active' || im.status === 'active') ? '#34d399' : '#fb7185';
          nodes.push({
            id: nid,
            label: im.hostname || im.name || `implant-${i}`,
            sub: `${im.implantType} | ${im.transport || '?'}`,
            x: COL3,
            y,
            color: statusColor,
            type: im.implantType,
          });
          // Try to connect to a matching listener
          const transport = (im.transport || '').toLowerCase();
          const match = listenArr.find(l => transport.includes(l.protocol.toLowerCase()));
          if (match) {
            edges.push({ from: `listener-${match.id}`, to: nid, label: 'implant' });
          } else {
            edges.push({ from: 'root', to: nid, label: 'implant' });
          }
        });
        if (implants.length === 0) {
          nodes.push({ id: 'no-implant', label: 'No implants', x: COL3, y: 250, color: '#7895b8', type: 'empty' });
        }

        // Build SVG
        const nodeR = 28;
        let svg = `<div class="graph-toolbar"><button class="btn btn-ghost btn-sm" onclick="window.router.navigate('#/graph')">⟳ Refresh</button><span style="font-size:0.75rem;color:var(--text-dim)">${implants.length} implant(s), ${listenArr.length} listener(s)</span></div>`;
        svg += `<div class="graph-container"><svg viewBox="0 0 ${W} ${H}" xmlns="http://www.w3.org/2000/svg">`;

        // Grid background
        svg += `<defs><pattern id="grid" width="40" height="40" patternUnits="userSpaceOnUse"><path d="M 40 0 L 0 0 0 40" fill="none" stroke="rgba(56,189,248,0.04)" stroke-width="1"/></pattern></defs>`;
        svg += `<rect width="${W}" height="${H}" fill="url(#grid)" />`;

        // Edges
        edges.forEach(e => {
          const from = nodes.find(n => n.id === e.from);
          const to = nodes.find(n => n.id === e.to);
          if (!from || !to) return;
          svg += `<line x1="${from.x}" y1="${from.y}" x2="${to.x}" y2="${to.y}" class="graph-edge active" />`;
          // midpoint label
          const mx = (from.x + to.x) / 2;
          const my = (from.y + to.y) / 2 - 8;
          svg += `<text x="${mx}" y="${my}" text-anchor="middle" fill="var(--text-dim)" font-size="9" font-family="var(--font-mono)">${escapeHtml(e.label)}</text>`;
        });

        // Nodes
        nodes.forEach(n => {
          const r = n.type === 'server' ? 32 : 24;
          const glow = n.color;
          svg += `<g class="graph-node" data-id="${n.id}">`;
          svg += `<circle cx="${n.x}" cy="${n.y}" r="${r}" fill="none" stroke="${n.color}" stroke-width="2" stroke-opacity="0.8" style="filter:drop-shadow(0 0 6px ${glow}40)" />`;
          svg += `<circle cx="${n.x}" cy="${n.y}" r="${r-4}" fill="${n.color}15" stroke="none" />`;
          // Label below
          svg += `<text x="${n.x}" y="${n.y + r + 14}" text-anchor="middle" class="graph-node-label" fill="${n.color}">${escapeHtml(n.label)}</text>`;
          if (n.sub) {
            svg += `<text x="${n.x}" y="${n.y + r + 28}" text-anchor="middle" font-size="8" fill="var(--text-dim)" font-family="var(--font-mono)">${escapeHtml(n.sub)}</text>`;
          }
          svg += `</g>`;
        });

        svg += '</svg></div>';
        main.innerHTML = svg;
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    };
    renderGraph();
    return () => { cancelled = true; };
  });
```

- [ ] **Step 2: Commit**

```bash
cd /Users/dsi/projects/naughtywolf && git add static/app.js && git commit -m "feat: add chain graph visualization SPA page with SVG renderer"
```

---
### Task 8: SPA shell HTML with navigation and connection status

**Files:**
- Modify: `static/app.html` (add app shell markup)
- Modify: `static/app.js` (update connection badge on each navigation)

- [ ] **Step 1: Rewrite `static/app.html` with full SPA shell**

```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>NaughtyWolf</title>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
<div class="app-shell">
    <nav class="side-rail">
        <div class="brand-block">
            <h1>NW</h1>
            <div class="brand-sub">Operator Console</div>
            <div class="conn-badge offline" id="conn-badge">checking...</div>
        </div>
        <div class="nav-section-title">Modules</div>
        <ul class="nav-list">
            <li class="nav-link" data-route="/dashboard" onclick="window.router.navigate('#/dashboard')">
                <span class="glyph">◈</span><span>Dashboard</span>
            </li>
            <li class="nav-link" data-route="/graph" onclick="window.router.navigate('#/graph')">
                <span class="glyph">◉</span><span>Chain Graph</span>
            </li>
            <li class="nav-link" data-route="/sessions" onclick="window.router.navigate('#/sessions')">
                <span class="glyph">⊞</span><span>Sessions</span>
            </li>
            <li class="nav-link" data-route="/beacons" onclick="window.router.navigate('#/beacons')">
                <span class="glyph">◇</span><span>Beacons</span>
            </li>
            <li class="nav-link" data-route="/listeners" onclick="window.router.navigate('#/listeners')">
                <span class="glyph">▽</span><span>Listeners</span>
            </li>
            <li class="nav-link" data-route="/payloads" onclick="window.router.navigate('#/payloads')">
                <span class="glyph">▣</span><span>Payloads</span>
            </li>
            <li class="nav-link" data-route="/websites" onclick="window.router.navigate('#/websites')">
                <span class="glyph">◎</span><span>Websites</span>
            </li>
            <li class="nav-link" data-route="/loot" onclick="window.router.navigate('#/loot')">
                <span class="glyph">♦</span><span>Loot</span>
            </li>
            <li class="nav-link" data-route="/creds" onclick="window.router.navigate('#/creds')">
                <span class="glyph">⚷</span><span>Credentials</span>
            </li>
            <li class="nav-link" data-route="/events" onclick="window.router.navigate('#/events')">
                <span class="glyph">⚡</span><span>Events</span>
            </li>
            <li class="nav-link" data-route="/audit" onclick="window.router.navigate('#/audit')">
                <span class="glyph">☰</span><span>Audit</span>
            </li>
        </ul>
        <div class="nav-section-title">System</div>
        <ul class="nav-list">
            <li class="nav-link" data-route="/admin" onclick="window.router.navigate('#/admin')">
                <span class="glyph">⚙</span><span>Admin</span>
            </li>
        </ul>
        <div class="side-rail-footer">
            <a href="/logout" style="color:var(--red);font-size:0.75rem;">Logout</a>
        </div>
    </nav>
    <div class="main-panel">
        <header class="status-strip">
            <div class="status-strip-left">
                <span class="page-title" id="page-title">Dashboard</span>
            </div>
            <div class="status-strip-right">
                <span class="user-badge" id="user-badge">—</span>
            </div>
        </header>
        <div class="content-area" id="main-content">
            <div class="loading"><div class="spinner"></div></div>
        </div>
    </div>
</div>
<script src="/static/app.js"></script>
</body>
</html>
```

- [ ] **Step 2: Update user badge on page load**

In `static/app.js`, after route registration, add DOMContentLoaded handler:

```javascript
// DOM is already loaded when app.js runs (script at end of body)
// Get user info from cookie or API
(async function init() {
  try {
    const status = await apiGet('/api/sliver/status');
    const badge = document.getElementById('conn-badge');
    if (badge && status) {
      badge.className = status.connected ? 'conn-badge online' : 'conn-badge offline';
      badge.textContent = status.connected ? (status.profile_name || 'connected') : 'disconnected';
    }
  } catch { /* ignore */ }

  try {
    // Try to find user info from page context
    const resp = await fetch('/api/users', { credentials: 'same-origin' });
    if (resp.ok) {
      // User is logged in, but this endpoint is admin-only — just indicate authenticated
    }
  } catch { /* ignore */ }

  // Navigate on init
  window.router.navigate(window.location.hash || '#/dashboard');
})();
```

Also, the initial `DOMContentLoaded` and `hashchange` listeners from the router core should be replaced — the init function handles first navigation.

Remove (or comment out) the old DOMContentLoaded handler from the router section:

```javascript
// Keep hashchange handler
window.addEventListener('hashchange', () => navigate(window.location.hash));

// Remove the DOMContentLoaded handler — init() handles first load
// window.addEventListener('DOMContentLoaded', () => navigate(window.location.hash || '#/dashboard'));
```

- [ ] **Step 3: Verify compilation**

```bash
cd /Users/dsi/projects/naughtywolf && cargo check
```
Expected: clean

- [ ] **Step 4: Commit**

```bash
cd /Users/dsi/projects/naughtywolf && git add static/app.html static/app.js && git commit -m "feat: complete SPA shell with navigation and connection status"
```

---
### Task 9: Run automated checks and verify dev server

- [ ] **Step 1: Run cargo check**

```bash
cd /Users/dsi/projects/naughtywolf && cargo check 2>&1
```
Expected: clean

- [ ] **Step 2: Run cargo test**

```bash
cd /Users/dsi/projects/naughtywolf && cargo test 2>&1
```
Expected: 23 passed, 0 failed

- [ ] **Step 3: Run cargo clippy**

```bash
cd /Users/dsi/projects/naughtywolf && cargo clippy 2>&1
```
Expected: clean

- [ ] **Step 4: Verify dev server is still running**

```bash
curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/login
```
Expected: 200

- [ ] **Step 5: Manual browser smoke check**

Open `http://127.0.0.1:8080/app` after login and verify:
- SPA shell renders (sidebar, topbar, content area)
- Navigation between all pages works without page reload
- Dashboard shows metric cards and event feed
- Chain Graph renders with status-connected state
- Sessions/Beacons/Listeners show data or empty state
- Payloads page loads
- Events page shows stream or empty state
- Admin shows users or empty state
- Logout link works (redirects to /login)
- Browser console has no fatal JS errors
- Login page restyled with new theme

- [ ] **Step 6: Fix any issues and commit**

```bash
git add -A
git commit -m "fix: address SPA migration issues from smoke check"
```
