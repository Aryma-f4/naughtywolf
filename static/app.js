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
  // Init handles first navigation via the async init block below.
  // window.addEventListener('DOMContentLoaded', () => navigate(window.location.hash || '#/dashboard'));

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

        const nodes = [];
        const edges = [];

        const rootLabel = status.connected ? (status.profile_name || 'Sliver Server') : 'Sliver (disconnected)';
        nodes.push({ id: 'root', label: rootLabel, x: COL1, y: 250, color: status.connected ? '#38bdf8' : '#7895b8', type: 'server' });

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

        let svg = `<div class="graph-toolbar"><button class="btn btn-ghost btn-sm" onclick="window.router.navigate('#/graph')">⟳ Refresh</button><span style="font-size:0.75rem;color:var(--text-dim)">${implants.length} implant(s), ${listenArr.length} listener(s)</span></div>`;
        svg += `<div class="graph-container"><svg viewBox="0 0 ${W} ${H}" xmlns="http://www.w3.org/2000/svg">`;
        svg += `<defs><pattern id="grid" width="40" height="40" patternUnits="userSpaceOnUse"><path d="M 40 0 L 0 0 0 40" fill="none" stroke="rgba(56,189,248,0.04)" stroke-width="1"/></pattern></defs>`;
        svg += `<rect width="${W}" height="${H}" fill="url(#grid)" />`;

        edges.forEach(e => {
          const from = nodes.find(n => n.id === e.from);
          const to = nodes.find(n => n.id === e.to);
          if (!from || !to) return;
          svg += `<line x1="${from.x}" y1="${from.y}" x2="${to.x}" y2="${to.y}" class="graph-edge active" />`;
          const mx = (from.x + to.x) / 2;
          const my = (from.y + to.y) / 2 - 8;
          svg += `<text x="${mx}" y="${my}" text-anchor="middle" fill="var(--text-dim)" font-size="9" font-family="var(--font-mono)">${escapeHtml(e.label)}</text>`;
        });

        nodes.forEach(n => {
          const r = n.type === 'server' ? 32 : 24;
          const glow = n.color;
          svg += `<g class="graph-node" data-id="${n.id}">`;
          svg += `<circle cx="${n.x}" cy="${n.y}" r="${r}" fill="none" stroke="${n.color}" stroke-width="2" stroke-opacity="0.8" style="filter:drop-shadow(0 0 6px ${glow}40)" />`;
          svg += `<circle cx="${n.x}" cy="${n.y}" r="${r-4}" fill="${n.color}15" stroke="none" />`;
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
})();

// ── Init ───────────────────────────────────────────────────
(async function init() {
  try {
    const status = await apiGet('/api/sliver/status');
    const badge = document.getElementById('conn-badge');
    if (badge && status) {
      badge.className = status.connected ? 'conn-badge online' : 'conn-badge offline';
      badge.textContent = status.connected ? (status.profile_name || 'connected') : 'disconnected';
    }
  } catch { /* ignore */ }

  // Navigate on init
  window.router.navigate(window.location.hash || '#/dashboard');
})();
