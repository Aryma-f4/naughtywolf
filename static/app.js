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
