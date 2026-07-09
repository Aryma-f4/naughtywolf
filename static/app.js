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

  // ── Toast / Notification system ──────────────────────────

  const toastContainer = document.createElement('div');
  toastContainer.style.cssText = 'position:fixed;top:16px;right:16px;z-index:9999;display:flex;flex-direction:column;gap:8px;';
  document.addEventListener('DOMContentLoaded', () => document.body.appendChild(toastContainer));

  window.showToast = function(msg, type) {
    type = type || 'info';
    const el = document.createElement('div');
    el.style.cssText = `padding:10px 16px;border-radius:6px;font-size:0.8rem;background:var(--bg-panel);border:1px solid;min-width:280px;box-shadow:0 4px 16px rgba(0,0,0,0.4);transition:opacity 0.3s;font-family:var(--font-mono)`;
    if (type === 'error') el.style.borderColor = 'var(--red)'; el.style.color = 'var(--red)';
    if (type === 'success') { el.style.borderColor = 'var(--green)'; el.style.color = 'var(--green)'; }
    if (type === 'info') { el.style.borderColor = 'var(--cyan)'; el.style.color = 'var(--cyan-soft)'; }
    el.textContent = msg;
    toastContainer.appendChild(el);
    setTimeout(() => { el.style.opacity = '0'; setTimeout(() => el.remove(), 300); }, 4000);
  };

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

  // ── Settings / Connect ──────────────────────────────────
  window.router.register('/settings', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        const status = await apiGet('/api/sliver/status').catch(() => ({connected:false}));
        if (cancelled) return;
        const connState = status.connected
          ? `<div class="notice notice-success">Connected — ${escapeHtml(status.profile_name || '')} (${escapeHtml(status.operator || '')}@${escapeHtml(status.lhost || '')})</div>`
          : '<div class="notice notice-info">Not connected to any Sliver server.</div>';
        const formHtml = status.connected
          ? `<button class="btn btn-danger" id="btn-disconnect">Disconnect</button>`
          : `<div class="field"><label>Config Path</label><input class="input" id="cfg-path" placeholder="/path/to/operator.cfg"></div>
             <div><button class="btn btn-primary" id="btn-connect">Connect</button></div>`;
        main.innerHTML = `
          <div class="panel">
            <div class="panel-header"><h3>Sliver Connection</h3></div>
            <div class="panel-body">
              ${connState}
              <div style="display:flex;gap:12px;margin-top:12px;">${formHtml}</div>
              <div id="conn-status" style="margin-top:12px;"></div>
            </div>
          </div>
          <div class="panel">
            <div class="panel-header"><h3>About</h3></div>
            <div class="panel-body" style="font-size:0.8rem;color:var(--text-muted);line-height:1.8;">
              NaughtyWolf v0.1.0 — Sliver C2 Web Console<br>
              Connect to a Sliver server to manage listeners, implants, and payloads.
            </div>
          </div>`;
        document.getElementById('btn-connect')?.addEventListener('click', async () => {
          const path = document.getElementById('cfg-path')?.value;
          if (!path) return;
          const st = document.getElementById('conn-status');
          st.innerHTML = '<span style="color:var(--cyan)">Connecting...</span>';
          try {
            const r = await apiPost('/api/sliver/connect', {config_path: path});
            showToast('Connected to Sliver server', 'success');
            window.router.navigate('#/settings');
          } catch(e) {
            st.innerHTML = `<span style="color:var(--red)">${escapeHtml(e.message)}</span>`;
          }
        });
        document.getElementById('btn-disconnect')?.addEventListener('click', async () => {
          try {
            await apiPost('/api/sliver/disconnect', {});
            showToast('Disconnected', 'info');
            window.router.navigate('#/settings');
          } catch(e) { showToast(e.message, 'error'); }
        });
      } catch(e) {
        if (!cancelled) main.innerHTML = window.renderError(e.message);
      }
    })();
    return () => { cancelled = true; };
  });

  // ── Chain Graph ──────────────────────────────────────────
  
  window.router.register('/graph', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    const renderGraph = async () => {
      try {
        const [status, listeners, sessions, beacons] = await Promise.all([
          apiGet('/api/sliver/status').catch(() => ({connected:false})),
          apiGet('/api/listeners').catch(() => []),
          apiGet('/api/sessions').catch(() => []),
          apiGet('/api/beacons').catch(() => []),
        ]);
        if (cancelled) return;

        const W = 900, H = 500;
        const COL1 = 150, COL2 = 350, COL3 = 600;

        const nodes = [{ id:'root', label:status.connected ? (status.profile_name||'Sliver Server') : 'Sliver (disconnected)', x:COL1, y:250, color:status.connected?'#38bdf8':'#7895b8', type:'server' }];
        const la = Array.isArray(listeners) ? listeners : [];
        la.forEach((l,i) => nodes.push({ id:'l-'+l.id, label:l.protocol+':'+l.port, sub:l.bind, x:COL2, y:100+i*80, color:'#34d399', type:'listener' }));
        if (!la.length) nodes.push({ id:'nl', label:'No listeners', x:COL2, y:250, color:'#7895b8', type:'empty' });

        const imps = [];
        (Array.isArray(sessions)?sessions:[]).forEach(s => imps.push({id:'im-'+imps.length, label:s.hostname||s.name, sub:'session | '+(s.transport||'?'), x:COL3, y:60+imps.length*60, color:s.status==='Active'?'#34d399':'#fb7185', type:'session' }));
        (Array.isArray(beacons)?beacons:[]).forEach(b => imps.push({id:'im-'+imps.length, label:b.hostname||b.name, sub:'beacon | '+(b.transport||'?'), x:COL3, y:60+imps.length*60, color:b.status==='Active'?'#34d399':'#fb7185', type:'beacon' }));
        if (!imps.length) nodes.push({ id:'ni', label:'No implants', x:COL3, y:250, color:'#7895b8', type:'empty' });

        var nm = {}; nodes.forEach(function(n){nm[n.id]=n;});

        var svg = '<div class="graph-toolbar"><button class="btn btn-ghost btn-sm" id="g-refresh">\u21bb Refresh</button><span style="font-size:0.75rem;color:var(--text-dim)">' + imps.length + ' implant(s), ' + la.length + ' listener(s)</span></div>';
        svg += '<div class="graph-container"><svg viewBox="0 0 900 500" xmlns="http://www.w3.org/2000/svg">';
        svg += '<defs><pattern id="g" width="40" height="40" patternUnits="userSpaceOnUse"><path d="M 40 0 L 0 0 0 40" fill="none" stroke="rgba(56,189,248,0.04)" stroke-width="1"/></pattern></defs>';
        svg += '<rect width="900" height="500" fill="url(#g)"/>';

        // Edges
        la.forEach(function(l) {
          var from = nm['root'], to = nm['l-'+l.id];
          if (from && to) { svg += '<line x1="'+from.x+'" y1="'+from.y+'" x2="'+to.x+'" y2="'+to.y+'" class="graph-edge active" />'; }
        });
        imps.forEach(function(im) {
          var from = nm['root'], to = nm[im.id];
          if (from && to) { svg += '<line x1="'+from.x+'" y1="'+from.y+'" x2="'+to.x+'" y2="'+to.y+'" class="graph-edge active" />'; }
        });

        // Nodes
        nodes.forEach(function(n) {
          var r = n.type === 'server' ? 32 : 24;
          svg += '<g class="graph-node" data-id="'+n.id+'">';
          svg += '<circle cx="'+n.x+'" cy="'+n.y+'" r="'+r+'" fill="none" stroke="'+n.color+'" stroke-width="2" stroke-opacity="0.8" style="filter:drop-shadow(0 0 6px '+n.color+'40)" />';
          svg += '<circle cx="'+n.x+'" cy="'+n.y+'" r="'+(r-4)+'" fill="'+n.color+'15" stroke="none" />';
          svg += '<text x="'+n.x+'" y="'+(n.y+r+14)+'" text-anchor="middle" class="graph-node-label" fill="'+n.color+'">'+escapeHtml(n.label)+'</text>';
          if (n.sub) svg += '<text x="'+n.x+'" y="'+(n.y+r+28)+'" text-anchor="middle" font-size="8" fill="var(--text-dim)" font-family="var(--font-mono)">'+escapeHtml(n.sub)+'</text>';
          svg += '</g>';
        });

        svg += '</svg></div><div id="gd" class="graph-details" style="display:none;"></div>';
        main.innerHTML = svg;

        // Hover via event delegation
        main.querySelector('.graph-container')?.addEventListener('mouseover', function(ev) {
          var g = ev.target.closest('.graph-node');
          if (!g) { document.getElementById('gd').style.display = 'none'; return; }
          var id = g.dataset.id;
          var n = nm[id];
          if (!n) return;
          var d = document.getElementById('gd');
          d.innerHTML = '<h4>'+escapeHtml(n.label)+'</h4><div class="detail-row"><span>Type</span><span>'+escapeHtml(n.type)+'</span></div><div class="detail-row"><span>ID</span><span>'+escapeHtml(n.id)+'</span></div>' + (n.sub ? '<div class="detail-row"><span>Info</span><span>'+escapeHtml(n.sub)+'</span></div>' : '');
          d.style.display = 'block';
        });
        main.querySelector('.graph-container')?.addEventListener('mouseout', function(ev) {
          if (ev.target.closest('.graph-node')) return;
          document.getElementById('gd').style.display = 'none';
        });
        document.getElementById('g-refresh')?.addEventListener('click', function() { window.router.navigate('#/graph'); });
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    };
    renderGraph();
    return function() { cancelled = true; };
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
    const loadListeners = async () => {
      try {
        const l = await apiGet('/api/listeners');
        if (cancelled) return '';
        if (!l || l.length === 0) return '<div class="empty-state"><div class="empty-icon">👂</div><h3>No Listeners</h3><p>Start a listener to see it here.</p></div>';
        const rows = l.map(lst => [
          escapeHtml(lst.id),
          escapeHtml(lst.protocol),
          escapeHtml(lst.bind),
          `<span class="badge badge-active">${escapeHtml(lst.status)}</span>`,
          `<button class="btn btn-danger btn-sm" data-kill="${lst.id}">Kill</button>`,
        ]);
        const table = window.renderTable(['ID','Protocol','Bind','Status','Actions'], rows);
        return `<div class="panel"><div class="panel-header"><h3>Active Listeners (${l.length})</h3></div><div class="panel-body">${table}</div></div>`;
      } catch { return '<div class="empty-state"><div class="empty-icon">⚠</div><h3>Error</h3><p>Failed to load listeners.</p></div>'; }
    };
    (async () => {
      const createForm = `
<div class="panel" style="margin-bottom:1rem;">
  <div class="panel-header"><h3>Start Listener</h3></div>
  <div class="panel-body">
    <form id="listener-form" class="form-grid">
      <div class="field"><label>Protocol</label><select class="select" id="lst-proto"><option value="mtls">mTLS</option><option value="http">HTTP</option></select></div>
      <div class="field"><label>Host</label><input class="input" id="lst-host" value="0.0.0.0"></div>
      <div class="field"><label>Port</label><input class="input" id="lst-port" type="number" value="8888"></div>
      <div class="field"><label>Domain</label><input class="input" id="lst-domain" placeholder="optional"></div>
      <div class="field full" style="grid-column:span 2;"><button type="submit" class="btn btn-primary">Start</button><span id="lst-msg" style="margin-left:12px;font-size:0.8rem;"></span></div>
    </form>
  </div>
</div>`;
      const tableHtml = await loadListeners();
      if (cancelled) return;
      main.innerHTML = createForm + tableHtml;
      document.getElementById('listener-form')?.addEventListener('submit', async (e) => {
        e.preventDefault();
        const msg = document.getElementById('lst-msg');
        msg.textContent = 'Starting...'; msg.style.color = 'var(--cyan)';
        try {
          const r = await apiPost('/api/listeners', {
            protocol: document.getElementById('lst-proto').value,
            host: document.getElementById('lst-host').value,
            port: parseInt(document.getElementById('lst-port').value),
            domain: document.getElementById('lst-domain').value || null,
          });
          if (r.success) { showToast(r.message, 'success'); window.router.navigate('#/listeners'); }
          else { msg.textContent = r.message; msg.style.color = 'var(--red)'; }
        } catch(e) { msg.textContent = e.message; msg.style.color = 'var(--red)'; }
      });
      // Kill buttons (delegation)
      main.addEventListener('click', async (e) => {
        const btn = e.target.closest('[data-kill]');
        if (!btn) return;
        const id = btn.dataset.kill;
        try {
          await apiPost('/api/listeners/kill/' + id, {});
          showToast('Listener ' + id + ' killed', 'info');
          window.router.navigate('#/listeners');
        } catch(e) { showToast(e.message, 'error'); }
      });
    })();
    return () => { cancelled = true; };
  });

  // ── Payloads ─────────────────────────────────────────────
  window.router.register('/payloads', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    const loadPayloads = async () => {
      try {
        const builds = await apiGet('/api/payloads');
        if (cancelled) return '';
        const fmtNames = ['executable','shared lib','shellcode','service'];
        const rows = (builds || []).map(b => [
          escapeHtml(b.name),
          `${escapeHtml(b.goos)}/${escapeHtml(b.goarch)}`,
          b.is_beacon ? 'beacon' : 'session',
          fmtNames[b.format] || 'unknown',
          `<a href="/api/payloads/download/${encodeURIComponent(b.name)}" class="btn btn-ghost btn-sm" target="_blank">Download</a>`,
        ]);
        return rows.length > 0
          ? window.renderTable(['Name','OS/Arch','Type','Format','Actions'], rows)
          : '<div class="empty-state"><div class="empty-icon">📦</div><h3>No Payloads</h3><p>Generate a payload to see it here.</p></div>';
      } catch { return '<div class="empty-state"><div class="empty-icon">⚠</div><h3>Error</h3><p>Failed to load payloads.</p></div>'; }
    };
    (async () => {
      const genForm = `
<div class="panel" style="margin-bottom:1rem;">
  <div class="panel-header"><h3>Generate Implant</h3></div>
  <div class="panel-body">
    <form id="gen-form" class="form-grid">
      <div class="field"><label>Name</label><input class="input" id="gen-name" placeholder="myimplant"></div>
      <div class="field"><label>Protocol</label><select class="select" id="gen-proto"><option value="http">HTTP</option><option value="https">HTTPS</option><option value="mtls">mTLS</option><option value="dns">DNS</option></select></div>
      <div class="field"><label>OS</label><select class="select" id="gen-os"><option value="linux">Linux</option><option value="windows">Windows</option><option value="darwin">macOS</option></select></div>
      <div class="field"><label>LHost</label><input class="input" id="gen-lhost" placeholder="0.0.0.0"></div>
      <div class="field"><label>Arch</label><select class="select" id="gen-arch"><option value="amd64">amd64</option><option value="386">386</option><option value="arm64">arm64</option></select></div>
      <div class="field"><label>LPort</label><input class="input" id="gen-lport" type="number" value="443"></div>
      <div class="field"><label>Format</label><select class="select" id="gen-fmt"><option value="exe">EXE</option><option value="shared">Shared</option><option value="shellcode">Shellcode</option><option value="service">Service</option></select></div>
      <div class="field" style="justify-content:flex-end;"><label><input type="checkbox" id="gen-beacon"> Beacon mode</label></div>
      <div class="field full" style="grid-column:span 2;"><button type="submit" class="btn btn-primary">Generate</button><span id="gen-msg" style="margin-left:12px;font-size:0.8rem;"></span></div>
    </form>
  </div>
</div>`;
      const tableContent = await loadPayloads();
      if (cancelled) return;
      main.innerHTML = genForm + '<div class="panel"><div class="panel-header"><h3>Implant Builds</h3></div><div class="panel-body">' + tableContent + '</div></div>';
      document.getElementById('gen-form')?.addEventListener('submit', async (e) => {
        e.preventDefault();
        const msg = document.getElementById('gen-msg');
        msg.textContent = 'Generating...'; msg.style.color = 'var(--cyan)';
        try {
          const r = await apiPost('/api/payloads/generate', {
            name: document.getElementById('gen-name').value,
            goos: document.getElementById('gen-os').value,
            goarch: document.getElementById('gen-arch').value,
            format: document.getElementById('gen-fmt').value,
            is_beacon: document.getElementById('gen-beacon').checked,
            protocol: document.getElementById('gen-proto').value,
            lhost: document.getElementById('gen-lhost').value,
            lport: parseInt(document.getElementById('gen-lport').value),
          });
          if (r.success) {
            msg.innerHTML = 'Done! <a href="'+escapeHtml(r.output_path||'')+'" class="btn btn-sm btn-primary" style="margin-left:8px;text-decoration:none;" target="_blank">📥 Download</a>';
            msg.style.color = 'var(--green)';
            showToast(r.message, 'success');
            window.router.navigate('#/payloads');
          }
          else { msg.textContent = r.message; msg.style.color = 'var(--red)'; }
        } catch(e) { msg.textContent = e.message; msg.style.color = 'var(--red)'; }
      });
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

// ── Auto-refresh connection status every 15s ──────────────
setInterval(() => {
  document.getElementById('conn-badge')?.textContent === 'checking...' ? null : null;
  try {
    apiGet('/api/sliver/status').then(status => {
      const badge = document.getElementById('conn-badge');
      if (!badge) return;
      badge.className = status.connected ? 'conn-badge online' : 'conn-badge offline';
      badge.textContent = status.connected ? (status.profile_name || 'connected') : 'disconnected';
    }).catch(() => {});
  } catch(e) {}
}, 15000);
