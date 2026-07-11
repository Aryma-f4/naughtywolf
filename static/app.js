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

  window.router = {
    register,
    navigate: (hash) => {
      window.location.hash = hash;
      // Ensure rendering happens even if hash didn't change (e.g. page refresh)
      const path = hash.replace(/^#/, '') || '/dashboard';
      navigate(path);
    }
  };

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
  // Fallback: append immediately if body already exists
  if (document.body) document.body.appendChild(toastContainer);

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
  
    
  // ── Hosts ─────────────────────────────────────────────────
  window.router.register('/hosts', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    (async () => {
      try {
        var hosts = await apiGet('/api/hosts');
        if (cancelled) return;
        if (!hosts || hosts.length === 0) {
          main.innerHTML = window.renderEmpty('💻', 'No Hosts', 'Connect to Sliver and wait for agents to check in.');
          return;
        }
        var rows = hosts.map(function(h) {
          return [escapeHtml(h.hostname), escapeHtml(h.os)+'/'+escapeHtml(h.arch), escapeHtml(h.transport), escapeHtml(h.remote_addr), '<span class="badge badge-active">'+h.agent_count+' agent(s)</span>'];
        });
        main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Hosts ('+hosts.length+')</h3></div><div class="panel-body">'
          + window.renderTable(['Hostname','OS/Arch','Transport','Remote','Agents'], rows)
          + '</div></div>';
      } catch(e) { if (!cancelled) main.innerHTML = window.renderError(e.message); }
    })();
    return function() { cancelled = true; };
  });
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

        var W = 900, H = 500;
        var COL1 = 150, COL2 = 350, COL3 = 600;

        var nodes = [{ id:'root', label:status.connected?(status.profile_name||'Sliver Server'):'Sliver (disconnected)', x:COL1, y:250, color:status.connected?'#38bdf8':'#7895b8', type:'server', os:'' }];
        var la = Array.isArray(listeners)?listeners:[];
        la.forEach(function(l,i){ nodes.push({ id:'l-'+l.id, label:l.protocol+':'+l.port, sub:l.bind, x:COL2, y:100+i*80, color:'#34d399', type:'listener', os:'' }); });
        if(!la.length) nodes.push({ id:'nl', label:'No listeners', x:COL2, y:250, color:'#7895b8', type:'empty', os:'' });

        var imps = [];
        (Array.isArray(sessions)?sessions:[]).forEach(function(s){ imps.push({id:s.id?'s-'+s.id:'s-'+imps.length, label:s.hostname||s.name, sub:'session | '+(s.transport||'?'), x:COL3, y:60+imps.length*60, color:s.status==='Active'?'#34d399':'#fb7185', type:'session', os:s.goos||'linux' }); });
        (Array.isArray(beacons)?beacons:[]).forEach(function(b){ imps.push({id:b.id?'b-'+b.id:'b-'+imps.length, label:b.hostname||b.name, sub:'beacon | '+(b.transport||'?'), x:COL3, y:60+imps.length*60, color:b.status==='Active'?'#34d399':'#fb7185', type:'beacon', os:b.goos||'linux' }); });
        if(!imps.length) nodes.push({ id:'ni', label:'No implants', x:COL3, y:250, color:'#7895b8', type:'empty', os:'' });

        var nm = {}; nodes.forEach(function(n){nm[n.id]=n;});

        var edges = [];
        la.forEach(function(l){ edges.push({ from:'root', to:'l-'+l.id }); });
        la.forEach(function(l){
          imps.forEach(function(im){
            if((im.sub||'').toLowerCase().indexOf(l.protocol)>=0) edges.push({ from:'l-'+l.id, to:im.id });
          });
        });
        imps.forEach(function(im){
          if(!edges.some(function(e){return e.to===im.id;})) edges.push({ from:'root', to:im.id });
        });

        function osI(n){
          if(!n||!n.os)return''; var o=n.os.toLowerCase();
          if(o.indexOf('win')>=0)return 'W'; if(o.indexOf('darwin')>=0||o.indexOf('mac')>=0)return 'A'; if(o.indexOf('linux')>=0)return 'L'; return'?';
        }

        var svg = '<div class="graph-toolbar"><button class="btn btn-ghost btn-sm" id="g-refresh">↻ Refresh</button><span style="font-size:0.75rem;color:var(--text-dim)">'+imps.length+' implant(s), '+la.length+' listener(s)</span></div>';
        svg += '<div class="graph-container"><svg viewBox="0 0 900 500" xmlns="http://www.w3.org/2000/svg">';
        svg += '<defs><pattern id="g" width="40" height="40" patternUnits="userSpaceOnUse"><path d="M 40 0 L 0 0 0 40" fill="none" stroke="rgba(56,189,248,0.04)" stroke-width="1"/></pattern></defs>';
        svg += '<rect width="900" height="500" fill="url(#g)"/>';

        edges.forEach(function(e){
          var f=nm[e.from],t=nm[e.to];
          if(f&&t) svg += '<line x1="'+f.x+'" y1="'+f.y+'" x2="'+t.x+'" y2="'+t.y+'" class="graph-edge active" data-from="'+e.from+'" data-to="'+e.to+'" />';
        });

        nodes.forEach(function(n){
          var r=n.type==='server'?32:24;
          var icon=osI(n);
          svg += '<g class="graph-node" data-id="'+n.id+'" data-type="'+n.type+'" data-os="'+n.os+'">';
          svg += '<circle cx="'+n.x+'" cy="'+n.y+'" r="'+r+'" fill="none" stroke="'+n.color+'" stroke-width="2" stroke-opacity="0.8" style="filter:drop-shadow(0 0 6px '+n.color+'40)" />';
          svg += '<circle cx="'+n.x+'" cy="'+n.y+'" r="'+(r-4)+'" fill="'+n.color+'15" stroke="none" />';
          if(icon) svg += '<text x="'+n.x+'" y="'+(n.y+6)+'" text-anchor="middle" font-size="'+(r>28?16:12)+'" fill="'+n.color+'" font-weight="bold">'+icon+'</text>';
          svg += '<text x="'+n.x+'" y="'+(n.y+r+14)+'" text-anchor="middle" class="graph-node-label" fill="'+n.color+'">'+escapeHtml(n.label)+'</text>';
          if(n.sub) svg += '<text x="'+n.x+'" y="'+(n.y+r+28)+'" text-anchor="middle" font-size="8" fill="var(--text-dim)" font-family="var(--font-mono)">'+escapeHtml(n.sub)+'</text>';
          svg += '</g>';
        });

        svg += '</svg></div><div id="gd" class="graph-details" style="display:none;"></div>';
        main.innerHTML = svg;

        var container = main.querySelector('.graph-container');

        container?.addEventListener('mouseover', function(ev){
          var g = ev.target.closest('.graph-node');
          if(!g){ document.getElementById('gd').style.display='none'; return; }
          var n = nm[g.dataset.id];
          if(!n) return;
          document.getElementById('gd').innerHTML = '<h4>'+escapeHtml(n.label)+'</h4><div class="detail-row"><span>Type</span><span>'+escapeHtml(n.type)+'</span></div><div class="detail-row"><span>ID</span><span>'+escapeHtml(n.id)+'</span></div>'+(n.sub?'<div class="detail-row"><span>Info</span><span>'+escapeHtml(n.sub)+'</span></div>':'')+(n.os?'<div class="detail-row"><span>OS</span><span>'+escapeHtml(n.os)+'</span></div>':'');
          document.getElementById('gd').style.display='block';
        });
        container?.addEventListener('mouseout', function(ev){
          if(ev.target.closest('.graph-node')) return;
          document.getElementById('gd').style.display='none';
        });

        // Drag using data-from/data-to on edges (no position matching)
        var dragId=null, dragOffX=0, dragOffY=0, dragEl=null;
        container?.addEventListener('mousedown', function(ev){
          var g=ev.target.closest('.graph-node');
          if(!g||g.dataset.id==='root') return;
          dragId=g.dataset.id; dragEl=g;
          var cx=parseFloat(g.querySelector('circle').getAttribute('cx'));
          var cy=parseFloat(g.querySelector('circle').getAttribute('cy'));
          var rect=container.getBoundingClientRect();
          dragOffX=ev.clientX-rect.left-cx; dragOffY=ev.clientY-rect.top-cy;
        });
        container?.addEventListener('mousemove', function(ev){
          if(!dragId||!dragEl) return;
          var rect=container.getBoundingClientRect();
          var nx=ev.clientX-rect.left-dragOffX, ny=ev.clientY-rect.top-dragOffY;
          nx=Math.max(30,Math.min(870,nx)); ny=Math.max(30,Math.min(470,ny));
          var n=nm[dragId]; if(n){n.x=nx;n.y=ny;}
          // Update circles and text in this node
          dragEl.querySelectorAll('circle').forEach(function(el){el.setAttribute('cx',nx);el.setAttribute('cy',ny);});
          // Update text labels
          dragEl.querySelectorAll('text').forEach(function(t){
            var base=parseFloat(t.getAttribute('data-base-y')||t.getAttribute('y'));
            if(!t.getAttribute('data-base-y'))t.setAttribute('data-base-y',base);
            t.setAttribute('y',ny+(base-(n?n.y:0)));
          });
          // Update edges by data-from/data-to
          container.querySelectorAll('line[data-from="'+dragId+'"],line[data-to="'+dragId+'"]').forEach(function(line){
            var f=nm[line.getAttribute('data-from')], t=nm[line.getAttribute('data-to')];
            if(f){line.setAttribute('x1',f.x);line.setAttribute('y1',f.y);}
            if(t){line.setAttribute('x2',t.x);line.setAttribute('y2',t.y);}
          });
        });
        container?.addEventListener('mouseup', function(){dragId=null;dragEl=null;});
        container?.addEventListener('mouseleave', function(){dragId=null;dragEl=null;});

        document.getElementById('g-refresh')?.addEventListener('click', function(){window.router.navigate('#/graph');});
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    };
    renderGraph();
    return function() { cancelled = true; };
  });

  // ── Agents ────────────────────────────────────────────────
  window.router.register('/agents', function(main) {
    main.innerHTML = window.renderLoading();
    let cancelled = false;
    let agents = [];
    let filters = { hostname: '', type: '', status: '' };

    function renderFilteredTable() {
      var filtered = agents.filter(function(a) {
        if (filters.hostname && a.hostname.toLowerCase().indexOf(filters.hostname.toLowerCase()) < 0) return false;
        if (filters.type && a.type !== filters.type) return false;
        if (filters.status === 'active' && a.status !== 'active') return false;
        if (filters.status === 'dead' && !a.is_dead) return false;
        if (filters.status === 'stale' && (a.status === 'active' || a.is_dead)) return false;
        return true;
      });
      if (filtered.length === 0) {
        return '<div class="empty-state"><div class="empty-icon">⊞</div><h3>No matching agents</h3><p>Try adjusting your filters.</p></div>';
      }
      var rows = filtered.map(function(a) {
        var badge = a.status === 'active' ? '<span class="badge badge-active">Active</span>' : (a.is_dead ? '<span class="badge badge-dead">Dead</span>' : '<span class="badge badge-warning">Stale</span>');
        return [
          '<a href="#/agents/'+encodeURIComponent(a.id)+'">'+escapeHtml(a.name)+'</a>',
          escapeHtml(a.hostname),
          '<span class="badge '+(a.type==='session'?'badge-active':'badge-unknown')+'">'+escapeHtml(a.type)+'</span>',
          escapeHtml(a.transport),
          escapeHtml(a.os+'/'+a.arch),
          badge,
          escapeHtml(a.last_checkin),
        ];
      });
      return '<div style="max-height:600px;overflow-y:auto;">' + window.renderTable(['Name','Hostname','Type','Transport','OS/Arch','Status','Last Checkin'], rows) + '</div>';
    }

    function renderPage() {
      main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Agents ('+agents.length+')</h3></div><div class="panel-body">'
        + '<div class="filter-bar" style="display:flex;gap:8px;margin-bottom:12px;flex-wrap:wrap;">'
        + '<input class="input" id="filter-hostname" placeholder="Filter hostname..." style="flex:1;min-width:160px;">'
        + '<select class="select" id="filter-type"><option value="">All types</option><option value="session">Session</option><option value="beacon">Beacon</option></select>'
        + '<select class="select" id="filter-status"><option value="">All status</option><option value="active">Active</option><option value="stale">Stale</option><option value="dead">Dead</option></select>'
        + '</div><div id="agents-table-container">' + renderFilteredTable() + '</div></div></div>';

      document.getElementById('filter-hostname')?.addEventListener('input', function() {
        filters.hostname = this.value;
        var container = document.getElementById('agents-table-container');
        if (container) container.innerHTML = renderFilteredTable();
      });
      document.getElementById('filter-type')?.addEventListener('change', function() {
        filters.type = this.value;
        var container = document.getElementById('agents-table-container');
        if (container) container.innerHTML = renderFilteredTable();
      });
      document.getElementById('filter-status')?.addEventListener('change', function() {
        filters.status = this.value;
        var container = document.getElementById('agents-table-container');
        if (container) container.innerHTML = renderFilteredTable();
      });
    }

    (async () => {
      try {
        agents = await apiGet('/api/agents');
        window.__agentData = agents;
        if (cancelled) return;
        if (!agents || agents.length === 0) {
          main.innerHTML = window.renderEmpty('⊞', 'No Agents', 'Connect to Sliver and wait for implants to check in.');
          return;
        }
        renderPage();
      } catch (err) {
        if (!cancelled) main.innerHTML = window.renderError(err.message);
      }
    })();
    return function() { cancelled = true; };
  });

  // ── Agent Detail (dynamic route) ─────────────────────────
  
  // ── File Browser (added in Phase 2.2) ────────────────────
  window.apiGetBinary = async function(url) {
    const res = await fetch(url, { credentials: 'same-origin' });
    if (!res.ok) throw new Error(`Download failed: ${res.status}`);
    const blob = await res.blob();
    return new Uint8Array(await blob.arrayBuffer());
  };
function renderAgentDetail(agentId) {
    var main = document.getElementById('main-content');
    if (!main) return;
    main.innerHTML = window.renderLoading();
    (async function() {
      try {
        var agents = window.__agentData || await apiGet('/api/agents');
        window.__agentData = agents;
        var agent = (agents||[]).find(function(a) { return a.id === agentId || a.name === agentId; });
        if (!agent) { main.innerHTML = window.renderError('Agent not found'); return; }
        var info = '<div class="panel"><div class="panel-header"><h3>Agent: '+escapeHtml(agent.name)+'</h3>'
          + '<button class="btn btn-ghost btn-sm" id="agent-refresh">↻ Refresh</button></div>'
          + '<div class="panel-body" style="font-size:0.85rem;">'
          + '<div class="detail-row"><span>ID</span><span>'+escapeHtml(agent.id)+'</span></div>'
          + '<div class="detail-row"><span>Type</span><span class="badge '+(agent.type==='session'?'badge-active':'badge-unknown')+'">'+escapeHtml(agent.type)+'</span></div>'
          + '<div class="detail-row"><span>Status</span><span class="badge '+(agent.status==='active'?'badge-active':'badge-dead')+'">'+escapeHtml(agent.status)+'</span></div>'
          + '<div class="detail-row"><span>Hostname</span><span>'+escapeHtml(agent.hostname)+'</span></div>'
          + '<div class="detail-row"><span>User</span><span>'+escapeHtml(agent.username)+'</span></div>'
          + '<div class="detail-row"><span>OS/Arch</span><span>'+escapeHtml(agent.os)+'/'+escapeHtml(agent.arch)+'</span></div>'
          + '<div class="detail-row"><span>Transport</span><span>'+escapeHtml(agent.transport)+'</span></div>'
          + '<div class="detail-row"><span>Interval</span><span>'+(agent.interval != null ? escapeHtml(agent.interval)+'s' : '—')+'</span></div>'
          + '<div class="detail-row"><span>Jitter</span><span>'+(agent.jitter != null ? escapeHtml(agent.jitter)+'s' : '—')+'</span></div>'
          + '<div class="detail-row"><span>Remote</span><span>'+escapeHtml(agent.remote_addr)+'</span></div>'
          + '<div class="detail-row"><span>Last Checkin</span><span>'+escapeHtml(agent.last_checkin)+'</span></div>'
          + '</div></div>';
        var shell = '<div class="panel"><div class="panel-header"><h3>Shell (VPS)</h3></div><div class="panel-body">'
          + '<form id="agent-shell-form"><div class="field"><label>Command</label><input class="input" id="agent-shell-cmd" placeholder="ls -la" style="font-family:var(--font-mono)"></div>'
          + '<button type="submit" class="btn btn-primary btn-sm">Execute</button>'
          + '<div id="agent-shell-out" style="margin-top:8px;font-family:var(--font-mono);font-size:0.8rem;white-space:pre-wrap;background:var(--bg-app);padding:8px;border-radius:4px;min-height:30px;max-height:300px;overflow-y:auto;"></div></div></div>';
        var tasks = '<div class="panel"><div class="panel-header"><h3>Task History</h3></div><div class="panel-body" id="agent-tasks-body"><div class="empty-state"><div class="empty-icon">⊞</div><h3>No tasks recorded</h3></div></div></div>';
        var files = '<div class="panel"><div class="panel-header"><h3>File Browser</h3></div><div class="panel-body">'
          + '<form id="agent-fs-form" style="display:flex;gap:8px;margin-bottom:8px;">'
          + '<input class="input" id="agent-fs-path" placeholder="/tmp" value="/tmp" style="font-family:var(--font-mono);flex:1">'
          + '<button type="submit" class="btn btn-primary btn-sm">List</button>'
          + '<button type="button" class="btn btn-ghost btn-sm" id="agent-fs-refresh">↻</button>'
          + '</form>'
          + '<div id="agent-fs-out" style="font-family:var(--font-mono);font-size:0.8rem;background:var(--bg-app);padding:8px;border-radius:4px;min-height:60px;max-height:280px;overflow-y:auto;"></div>'
          + '<hr style="border-color:var(--border-subtle);margin:12px 0">'
          + '<form id="agent-fs-up-form" style="display:flex;gap:8px;align-items:flex-end;">'
          + '<div style="flex:1"><label style="font-size:0.7rem;color:var(--text-muted)">Upload Path</label>'
          + '<input class="input" id="agent-fs-up-path" placeholder="/tmp/file.txt" style="font-family:var(--font-mono)"></div>'
          + '<div style="flex:1"><label style="font-size:0.7rem;color:var(--text-muted)">File Name</label>'
          + '<input class="input" id="agent-fs-up-name" placeholder="upload.bin" style="font-family:var(--font-mono)"></div>'
          + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Content</label>'
          + '<input type="file" id="agent-fs-up-file" style="font-family:var(--font-mono)"></div>'
          + '<button type="submit" class="btn btn-primary btn-sm">Upload</button>'
          + '</form>'
          + '<div id="agent-fs-up-status" style="margin-top:6px;font-size:0.8rem;"></div>'
          + '</div></div>';
        main.innerHTML = info + shell + files + tasks;
        document.getElementById('agent-refresh')?.addEventListener('click', async function() {
          try {
            var fresh = await apiGet('/api/agents');
            window.__agentData = fresh;
            renderAgentDetail(agentId);
          } catch(e) { showToast(e.message, 'error'); }
        });
        document.getElementById('agent-shell-form')?.addEventListener('submit', async function(e) {
          e.preventDefault();
          var cmd = document.getElementById('agent-shell-cmd').value;
          if (!cmd) return;
          var out = document.getElementById('agent-shell-out');
          out.textContent = 'Running: '+cmd+'\n---\n';
          try {
            var r = await apiPost('/api/shell/exec', {command: cmd});
            out.textContent += r.stdout;
            if (r.stderr) out.textContent += '\nSTDERR:\n'+r.stderr;
            out.textContent += '\n---\nExit: '+r.exit_code;
          } catch(e) { out.textContent += '\nError: '+e.message; }
        });
        // File browser event handlers
        function listPath(path) {
          var out = document.getElementById('agent-fs-out');
          out.textContent = 'Listing ' + path + '...';
          apiPost('/api/agents/' + encodeURIComponent(agentId) + '/fs/ls', { path: path })
            .then(function(r) {
              if (!r || !r.exists) {
                out.textContent = 'Path does not exist: ' + path;
                return;
              }
              var rows = (r.files || []).map(function(f) {
                var sizeStr = f.is_dir ? '<dir>' : (f.size != null ? f.size + ' B' : '—');
                var name = f.name + (f.is_dir ? '/' : '');
                var dlBtn = f.is_dir ? '' : '<button class="btn btn-ghost btn-sm" data-dl="'+escapeHtml(f.path||name)+'">Download</button>';
                return ['<strong>' + escapeHtml(name) + '</strong>', sizeStr, escapeHtml(f.mod_time || '—'), escapeHtml(f.mode || '—'), dlBtn];
              });
              out.innerHTML = '<div style="margin-bottom:6px;">Path: <strong>' + escapeHtml(r.path) + '</strong> (' + rows.length + ' entries)</div>' +
                (rows.length ? window.renderTable(['Name','Size','Modified','Mode',''], rows) : '<div class="empty-state">Empty directory</div>');
              out.querySelectorAll('button[data-dl]').forEach(function(btn) {
                btn.addEventListener('click', function() {
                  var p = btn.getAttribute('data-dl');
                  window.location.href = '/api/agents/' + encodeURIComponent(agentId) + '/fs/download?path=' + encodeURIComponent(p);
                });
              });
            })
            .catch(function(e) { out.textContent = 'Error: ' + e.message; });
        }

        document.getElementById('agent-fs-form')?.addEventListener('submit', function(e) {
          e.preventDefault();
          var p = document.getElementById('agent-fs-path').value || '/tmp';
          listPath(p);
        });
        document.getElementById('agent-fs-refresh')?.addEventListener('click', function() {
          var p = document.getElementById('agent-fs-path').value || '/tmp';
          listPath(p);
        });

        // Initial load — list /tmp
        listPath('/tmp');

        document.getElementById('agent-fs-up-form')?.addEventListener('submit', function(e) {
          e.preventDefault();
          var f = document.getElementById('agent-fs-up-file');
          if (!f || !f.files || f.files.length === 0) { alert('Pick a file'); return; }
          var status = document.getElementById('agent-fs-up-status');
          status.textContent = 'Reading file...';
          var fr = new FileReader();
          fr.onload = async function(ev) {
            var data = new Uint8Array(ev.target.result);
            var arr = Array.from(data);
            var path = document.getElementById('agent-fs-up-path').value;
            var name = document.getElementById('agent-fs-up-name').value || f.files[0].name;
            try {
              var r = await apiPost('/api/agents/' + encodeURIComponent(agentId) + '/fs/upload', {
                path: path, file_name: name, data: arr, overwrite: false
              });
              status.textContent = '✅ Uploaded to ' + r.path;
            } catch(e) { status.textContent = '❌ ' + e.message; }
          };
          fr.readAsArrayBuffer(f.files[0]);
        });

        // Try to fetch task history if the endpoint exists
        try {
          var taskHistory = await apiGet('/api/agents/'+encodeURIComponent(agentId)+'/tasks');
          if (taskHistory && taskHistory.length > 0) {
            var tRows = taskHistory.map(function(t) {
              return [
                escapeHtml(t.id || '—'),
                escapeHtml(t.command || '—'),
                escapeHtml(t.status || '—'),
                t.exit_code != null ? escapeHtml(String(t.exit_code)) : '—',
                escapeHtml(t.executed_at || t.created_at || '—'),
              ];
            });
            document.getElementById('agent-tasks-body').innerHTML = window.renderTable(['ID','Command','Status','Exit Code','Executed At'], tRows);
          }
        } catch(e) { /* task endpoint not available */ }
      } catch(e) { main.innerHTML = window.renderError(e.message); }
    })();
  }

  // Override navigate to handle /agents/<id> routes
  (function() {
    var origNav = window.router.navigate;
    window.router.navigate = function(hash) {
      var path = hash.replace(/^#/,'')||'/dashboard';
      var m = path.match(/^\/agents\/(.+)$/);
      if (m && m[1]) { renderAgentDetail(decodeURIComponent(m[1])); return; }
      origNav(hash);
    };
  })();

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
        if (!confirm('Kill listener ' + btn.dataset.kill + '?')) return;
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
        const btn = e.target.querySelector('button[type="submit"]');
        msg.textContent = 'Starting...'; msg.style.color = 'var(--cyan)'; btn.disabled = true;
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
            msg.innerHTML = '<span style="color:var(--cyan)">⚙ Forging payload...</span>';
            msg.style.color = 'var(--cyan)';
            showToast('Forging payload: ' + document.getElementById('gen-name').value, 'success');
            // Auto-refresh payloads list every 8s until file appears or 3min
            var pName = document.getElementById('gen-name').value;
            var pCount = 0;
            var pTimer = setInterval(async function() {
              pCount++;
              try {
                var pBuilds = await apiGet('/api/payloads');
                var pFound = (pBuilds||[]).find(function(b) { return b.name.indexOf(pName) >= 0; });
                if (pFound || pCount > 20) {
                  clearInterval(pTimer);
                  msg.innerHTML = pFound ? '<span style="color:var(--green)">✅ Payload ready!</span>' : '<span style="color:var(--red)">❌ Timed out</span>';
                  btn.disabled = false;
                  btn.textContent = 'Generate';
                  window.router.navigate('#/payloads');
                } else {
                  msg.innerHTML = '<span style="color:var(--cyan)">⚙ Forging payload... ' + (pCount*8) + 's</span>';
                }
              } catch(e) {}
            }, 8000);
          } else {
            msg.textContent = r.message; msg.style.color = 'var(--red)'; btn.disabled = false;
          }
        } catch(e) { msg.textContent = e.message; msg.style.color = 'var(--red)'; btn.disabled = false; }
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

  // ── Shell ────────────────────────────────────────────────
  window.router.register('/shell', function(main) {
    main.innerHTML = '<div class="panel"><div class="panel-header"><h3>Shell</h3></div><div class="panel-body"><form id="shell-form"><div class="field"><label>Command</label><input class="input" id="shell-cmd" placeholder="ls -la" style="font-family:var(--font-mono)"></div><button type="submit" class="btn btn-primary" style="margin-top:8px">Run</button></form><div id="shell-out" style="margin-top:12px;font-family:var(--font-mono);font-size:0.8rem;white-space:pre-wrap;background:var(--bg-app);padding:8px;border-radius:4px;min-height:40px;max-height:400px;overflow-y:auto;"></div></div></div>';
    document.getElementById('shell-form')?.addEventListener('submit', async function(e) {
      e.preventDefault();
      var cmd = document.getElementById('shell-cmd').value;
      if (!cmd) return;
      var out = document.getElementById('shell-out');
      out.textContent = 'Running: ' + cmd + '\n---\n';
      try {
        var r = await apiPost('/api/shell/exec', { command: cmd });
        out.textContent += r.stdout;
        if (r.stderr) out.textContent += '\nSTDERR:\n' + r.stderr;
        out.textContent += '\n---\nExit code: ' + r.exit_code;
      } catch(e) { out.textContent += '\nError: ' + e.message; }
    });
  });

  // ── Credentials ──────────────────────────────────────────
  // ── Credentials (Empire-style management UI, Phase 2.3) ─────
  window.renderCredsPage = async function() {
    var main = document.getElementById('main-content');
    if (!main) return;
    var canEdit = window.__agentCanEdit !== false; // default true; server enforces RBAC
    var html = '<div class="panel">'
      + '<div class="panel-header"><h3>Credentials</h3>'
      + '<div style="display:flex;gap:8px;align-items:center">'
      + '<input class="input" id="cred-search" placeholder="Filter (user, domain, host)" style="font-size:0.8rem;max-width:280px">'
      + '<select class="select" id="cred-source-filter" style="font-size:0.8rem">'
      + '<option value="">All sources</option>'
      + '<option value="manual">Manual</option>'
      + '<option value="sliver">From Sliver</option>'
      + '</select>'
      + '<button class="btn btn-ghost btn-sm" id="cred-refresh">↻ Refresh</button>'
      + '</div>'
      + '</div>'
      + '<div class="panel-body">'
      + (canEdit
        ? '<form id="cred-add-form" style="display:grid;grid-template-columns:repeat(3,1fr);gap:8px;margin-bottom:12px;padding-bottom:12px;border-bottom:1px solid var(--border-subtle)">'
          + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Username</label><input class="input" id="cred-add-user" placeholder="admin"></div>'
          + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Domain</label><input class="input" id="cred-add-domain" placeholder="CORP.LOCAL"></div>'
          + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Password</label><input class="input" id="cred-add-pass" placeholder="P@ssw0rd!"></div>'
          + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Host</label><input class="input" id="cred-add-host" placeholder="DC01.corp.local"></div>'
          + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Hash Type</label><input class="input" id="cred-add-hashtype" placeholder="NTLMv1"></div>'
          + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Hash</label><input class="input" id="cred-add-hash" placeholder="aad3b435..."></div>'
          + '<div style="grid-column:span 3;display:flex;gap:8px;align-items:center">'
          + '<select class="select" id="cred-add-type" style="font-size:0.8rem"><option value="hash">Hash</option><option value="plaintext">Plaintext</option></select>'
          + '<select class="select" id="cred-add-source" style="font-size:0.8rem"><option value="manual">manual</option><option value="sliver">sliver</option></select>'
          + '<input class="input" id="cred-add-notes" placeholder="Notes (optional)" style="flex:1;font-size:0.8rem">'
          + '<button type="submit" class="btn btn-primary btn-sm">+ Add</button>'
          + '</div>'
          + '</form>'
        : '<div style="margin-bottom:8px;font-size:0.8rem;color:var(--text-muted)">Viewer role: read-only</div>')
      + '<div id="cred-list"></div>'
      + '</div></div>';
    main.innerHTML = html;

    function applyFilters(items) {
      var q = (document.getElementById('cred-search')?.value || '').toLowerCase().trim();
      var src = document.getElementById('cred-source-filter')?.value || '';
      return items.filter(function(c) {
        var hay = [c.id, c.username, c.collection, c.hash_type, c.plaintext].join(' ').toLowerCase();
        if (q && hay.indexOf(q) < 0) return false;
        if (src && c.collection !== src) return false;
        return true;
      });
    }

    function renderList(all) {
      var filtered = applyFilters(all);
      var list = document.getElementById('cred-list');
      if (!filtered.length) {
        list.innerHTML = '<div class="empty-state"><div class="empty-icon">🔑</div><h3>No credentials</h3><p>Add one above or pull from Sliver.</p></div>';
        return;
      }
      var rows = filtered.map(function(c) {
        var status = c.is_cracked
          ? '<span class="badge badge-active">cracked</span>'
          : '<span class="badge badge-warning">hash only</span>';
        var actions = '<button class="btn btn-ghost btn-sm" data-action="crack" data-id="' + escapeHtml(c.id) + '" data-plaintext="' + escapeHtml(c.plaintext) + '">Set plaintext</button>';
        if (canEdit) {
          actions += ' <button class="btn btn-ghost btn-sm" data-action="delete" data-id="' + escapeHtml(c.id) + '">Delete</button>';
        }
        return [
          escapeHtml((c.username || '—').split('@')[0]),
          escapeHtml(c.username || '—'),
          escapeHtml(c.plaintext || '—'),
          escapeHtml(c.hash_type || '—'),
          escapeHtml(c.collection || '—'),
          status,
          actions,
        ];
      });
      list.innerHTML = window.renderTable(['User','Username@Domain','Plaintext','Hash Type','Source','Status','Actions'], rows);
      list.querySelectorAll('button[data-action="delete"]').forEach(function(b) {
        b.addEventListener('click', async function() {
          if (!confirm('Delete credential ' + b.dataset.id + '?')) return;
          try {
            await fetch('/api/creds/' + encodeURIComponent(b.dataset.id), { method: 'DELETE', credentials: 'same-origin' });
            showToast('Credential deleted', 'success');
            load();
          } catch(e) { showToast(e.message, 'error'); }
        });
      });
      list.querySelectorAll('button[data-action="crack"]').forEach(function(b) {
        b.addEventListener('click', function() {
          var pt = prompt('Plaintext password for ' + b.dataset.id + '?', b.dataset.plaintext || '');
          if (pt == null) return;
          apiPost('/api/creds/' + encodeURIComponent(b.dataset.id) + '/crack', { plaintext: pt })
            .then(function() { showToast('Credential marked cracked', 'success'); load(); })
            .catch(function(e) { showToast(e.message, 'error'); });
        });
      });
    }

    function load() {
      apiGet('/api/creds').then(function(data) {
        window.__credData = data || [];
        renderList(window.__credData);
      }).catch(function(e) {
        document.getElementById('cred-list').innerHTML = '<div class="empty-state"><div class="empty-icon">⚠</div><h3>Error</h3><p>' + escapeHtml(e.message) + '</p></div>';
      });
    }

    document.getElementById('cred-search')?.addEventListener('input', function() {
      renderList(window.__credData || []);
    });
    document.getElementById('cred-source-filter')?.addEventListener('change', function() {
      renderList(window.__credData || []);
    });
    document.getElementById('cred-refresh')?.addEventListener('click', load);

    if (canEdit) {
      document.getElementById('cred-add-form')?.addEventListener('submit', function(e) {
        e.preventDefault();
        var req = {
          cred_type: document.getElementById('cred-add-type').value,
          username: document.getElementById('cred-add-user').value,
          domain: document.getElementById('cred-add-domain').value,
          password: document.getElementById('cred-add-pass').value,
          host: document.getElementById('cred-add-host').value,
          hash: document.getElementById('cred-add-hash').value,
          hash_type: document.getElementById('cred-add-hashtype').value,
          source: document.getElementById('cred-add-source').value,
          notes: document.getElementById('cred-add-notes').value,
        };
        apiPost('/api/creds', req).then(function() {
          showToast('Credential added', 'success');
          document.getElementById('cred-add-form').reset();
          load();
        }).catch(function(err) { showToast(err.message, 'error'); });
      });
    }

    load();
  };

  window.router.register('/creds', function(main) {
    window.renderCredsPage();
  });

  // ── Modules (Phase 2.4) ─────────────────────────────────
  window.renderModulesPage = async function() {
    var main = document.getElementById('main-content');
    if (!main) return;
    var html = '<div class="panel">'
      + '<div class="panel-header"><h3>Sliver Modules</h3>'
      + '<div style="display:flex;gap:8px;align-items:center">'
      + '<input class="input" id="mod-search" placeholder="Filter modules" style="font-size:0.8rem;max-width:280px">'
      + '<button class="btn btn-ghost btn-sm" id="mod-refresh">↻ Refresh</button>'
      + '</div>'
      + '</div>'
      + '<div class="panel-body">'
      + '<div class="form-grid" style="grid-template-columns:repeat(3,1fr);gap:8px;margin-bottom:12px;padding-bottom:12px;border-bottom:1px solid var(--border-subtle)">'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Target Session ID</label><input class="input" id="mod-session" placeholder="session-id"></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Module</label><select class="select" id="mod-name"></select></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Args (JSON)</label><input class="input" id="mod-args" placeholder=\'{"k":"v"}\'></div>'
      + '<div style="grid-column:span 3;display:flex;gap:8px;align-items:center">'
      + '<label style="font-size:0.75rem"><input type="checkbox" id="mod-srv"> Run on server (relay)</label>'
      + '<button type="button" class="btn btn-primary btn-sm" id="mod-exec">Execute</button>'
      + '<span id="mod-status" style="font-size:0.8rem;color:var(--text-muted)"></span>'
      + '</div>'
      + '</div>'
      + '<div id="mod-out" style="margin-top:8px;font-family:var(--font-mono);font-size:0.8rem;background:var(--bg-app);padding:8px;border-radius:4px;min-height:30px;max-height:240px;overflow-y:auto;white-space:pre-wrap"></div>'
      + '<div id="mod-list" style="margin-top:12px"></div>'
      + '</div></div>';
    main.innerHTML = html;

    function renderModList(items) {
      var q = (document.getElementById('mod-search')?.value || '').toLowerCase().trim();
      var filtered = q ? items.filter(function(m) { return m.name.toLowerCase().indexOf(q) >= 0; }) : items;
      var list = document.getElementById('mod-list');
      var sel = document.getElementById('mod-name');
      if (!filtered.length) {
        list.innerHTML = '<div class="empty-state"><div class="empty-icon">⌘</div><h3>No modules loaded</h3><p>Run <code>sliver-client armory install ...</code> on the server.</p></div>';
        sel.innerHTML = '<option value="">No modules</option>';
        return;
      }
      var rows = filtered.map(function(m) {
        var badge = m.server_store ? '<span class="badge badge-active">server</span>' : '<span class="badge badge-unknown">agent</span>';
        return ['<strong>' + escapeHtml(m.name) + '</strong>', badge];
      });
      list.innerHTML = '<h4 style="font-size:0.7rem;color:var(--text-muted);text-transform:uppercase;margin-bottom:6px">Available (' + filtered.length + ')</h4>'
        + window.renderTable(['Name', 'Source'], rows);
      sel.innerHTML = '<option value="">Select module...</option>'
        + filtered.map(function(m) { return '<option value="' + escapeHtml(m.name) + '">' + escapeHtml(m.name) + '</option>'; }).join('');
    }

    function load() {
      apiGet('/api/modules').then(function(data) {
        window.__modData = data || [];
        renderModList(window.__modData);
      }).catch(function(e) {
        document.getElementById('mod-list').innerHTML = '<div class="empty-state"><div class="empty-icon">⚠</div><h3>Error</h3><p>' + escapeHtml(e.message) + '</p></div>';
      });
    }

    document.getElementById('mod-search')?.addEventListener('input', function() {
      renderModList(window.__modData || []);
    });
    document.getElementById('mod-refresh')?.addEventListener('click', load);

    document.getElementById('mod-exec')?.addEventListener('click', function() {
      var sid = document.getElementById('mod-session').value.trim();
      var mod = document.getElementById('mod-name').value;
      var argsRaw = document.getElementById('mod-args').value.trim();
      var srv = document.getElementById('mod-srv').checked;
      var status = document.getElementById('mod-status');
      var out = document.getElementById('mod-out');
      if (!sid) { status.textContent = '⚠ Session ID required'; return; }
      if (!mod) { status.textContent = '⚠ Select a module'; return; }
      var body = { server_store: srv ? true : undefined, args: argsRaw || undefined };
      apiPost('/api/agents/' + encodeURIComponent(sid) + '/modules/' + encodeURIComponent(mod) + '/exec', body)
        .then(function(r) {
          status.textContent = '✅ ' + r.message;
          out.textContent = r.output || '(no output)';
        })
        .catch(function(err) {
          status.textContent = '❌ ' + err.message;
          out.textContent = '';
        });
    });

    load();
  };

  window.router.register('/modules', function(main) {
    window.renderModulesPage();
  });

  // ── Stagers (Phase 2.5) ─────────────────────────────────
  window.renderStagersPage = async function() {
    var main = document.getElementById('main-content');
    if (!main) return;
    var html = '<div class="panel">'
      + '<div class="panel-header"><h3>Stager Templates</h3>'
      + '<div style="display:flex;gap:8px;align-items:center">'
      + '<input class="input" id="stg-search" placeholder="Filter by name / OS / protocol" style="font-size:0.8rem;max-width:300px">'
      + '<button class="btn btn-ghost btn-sm" id="stg-refresh">↻ Refresh</button>'
      + '</div></div>'
      + '<div class="panel-body">'
      + '<form id="stg-add-form" style="display:grid;grid-template-columns:repeat(4,1fr);gap:6px;margin-bottom:12px;padding-bottom:12px;border-bottom:1px solid var(--border-subtle)">'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Name</label><input class="input" id="stg-add-name" required></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">OS</label><select class="select" id="stg-add-os"><option>linux</option><option>windows</option><option>darwin</option></select></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Arch</label><select class="select" id="stg-add-arch"><option>amd64</option><option>386</option><option>arm64</option></select></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Format</label><select class="select" id="stg-add-fmt"><option value="2">exe</option><option value="0">shared</option><option value="3">service</option><option value="1">shellcode</option></select></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Protocol</label><select class="select" id="stg-add-proto"><option>mtls</option><option>http</option><option>https</option><option>dns</option></select></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">LHost</label><input class="input" id="stg-add-lhost" placeholder="0.0.0.0"></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">LPort</label><input class="input" id="stg-add-lport" type="number" value="443"></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Note</label><input class="input" id="stg-add-desc"></div>'
      + '<div style="grid-column:span 4;display:flex;gap:8px;align-items:center">'
      + '<label style="font-size:0.75rem"><input type="checkbox" id="stg-add-beacon"> Beacon</label>'
      + '<label style="font-size:0.75rem"><input type="checkbox" id="stg-add-obfuscate" checked> Obfuscate</label>'
      + '<button type="submit" class="btn btn-primary btn-sm">+ Add Template</button>'
      + '<span id="stg-add-status" style="font-size:0.8rem;color:var(--text-muted)"></span>'
      + '</div>'
      + '</form>'
      + '<div id="stg-list"></div>'
      + '</div></div>';
    main.innerHTML = html;

    var FMT_NAMES = ['shared lib', 'shellcode', 'exe', 'service'];

    function renderStagers(items) {
      var q = (document.getElementById('stg-search')?.value || '').toLowerCase().trim();
      var filtered = q ? items.filter(function(s) {
        return (s.name + ' ' + s.goos + ' ' + s.protocol + ' ' + s.description).toLowerCase().indexOf(q) >= 0;
      }) : items;
      var list = document.getElementById('stg-list');
      if (!filtered.length) {
        list.innerHTML = '<div class="empty-state"><div class="empty-icon">📋</div><h3>No templates</h3><p>Add one above to get started.</p></div>';
        return;
      }
      var rows = filtered.map(function(s) {
        var fmtStr = FMT_NAMES[s.format] || 'unknown';
        var protoBadge = s.protocol === 'mtls' ? 'badge-active' : 'badge-warning';
        var beaconBadge = s.is_beacon ? '<span class="badge badge-active">beacon</span>' : '';
        var obfBadge = s.obfuscate ? '<span class="badge badge-active">obfuscate</span>' : '';
        var generateBtn = '<button class="btn btn-primary btn-sm" data-act="generate" data-id="' + s.id + '" data-name="' + escapeHtml(s.name) + '">Generate</button>';
        var deleteBtn = '<button class="btn btn-ghost btn-sm" data-act="delete" data-id="' + s.id + '">×</button>';
        return [
          '<strong>' + escapeHtml(s.name) + '</strong> ' + beaconBadge,
          '<span class="badge ' + protoBadge + '">' + escapeHtml(s.protocol) + '</span>',
          escapeHtml(s.goos) + '/' + escapeHtml(s.goarch),
          escapeHtml(fmtStr),
          s.sample_count,
          generateBtn + ' ' + deleteBtn,
        ];
      });
      list.innerHTML = window.renderTable(['Name','Proto','OS/Arch','Format','Uses','Action'], rows);

      list.querySelectorAll('button[data-act="delete"]').forEach(function(b) {
        b.addEventListener('click', async function() {
          if (!confirm('Delete template ' + b.dataset.id + '?')) return;
          try {
            await fetch('/api/stagers/' + b.dataset.id, { method: 'DELETE', credentials: 'same-origin' });
            showToast('Template deleted', 'success');
            load();
          } catch(e) { showToast(e.message, 'error'); }
        });
      });

      list.querySelectorAll('button[data-act="generate"]').forEach(function(b) {
        b.addEventListener('click', async function() {
          var lhost = prompt('LHost (e.g. 10.0.0.1)?', '127.0.0.1');
          if (!lhost) return;
          var lport = prompt('LPort (e.g. 443)?', '443');
          if (!lport) return;
          b.disabled = true;
          b.textContent = 'Generating…';
          try {
            var r = await apiPost('/api/stagers/' + b.dataset.id + '/generate', { lhost: lhost, lport: parseInt(lport) });
            if (r.success) {
              showToast(r.message, 'success');
              window.location.hash = '#/payloads';
            } else {
              showToast(r.message, 'error');
            }
          } catch(e) { showToast(e.message, 'error'); }
          b.disabled = false;
          b.textContent = 'Generate';
        });
      });
    }

    function load() {
      apiGet('/api/stagers').then(function(data) {
        window.__stgData = data || [];
        renderStagers(window.__stgData);
      }).catch(function(e) {
        document.getElementById('stg-list').innerHTML = '<div class="empty-state"><div class="empty-icon">⚠</div><h3>Error</h3><p>' + escapeHtml(e.message) + '</p></div>';
      });
    }

    document.getElementById('stg-search')?.addEventListener('input', function() { renderStagers(window.__stgData || []); });
    document.getElementById('stg-refresh')?.addEventListener('click', load);
    document.getElementById('stg-add-form')?.addEventListener('submit', function(e) {
      e.preventDefault();
      var body = {
        name: document.getElementById('stg-add-name').value,
        description: document.getElementById('stg-add-desc').value || undefined,
        goos: document.getElementById('stg-add-os').value,
        goarch: document.getElementById('stg-add-arch').value,
        format: parseInt(document.getElementById('stg-add-fmt').value),
        protocol: document.getElementById('stg-add-proto').value,
        is_beacon: document.getElementById('stg-add-beacon').checked || undefined,
        obfuscate: document.getElementById('stg-add-obfuscate').checked || undefined,
      };
      var status = document.getElementById('stg-add-status');
      status.textContent = 'Adding…';
      apiPost('/api/stagers', body)
        .then(function() { status.textContent = '✅ Added'; document.getElementById('stg-add-form').reset(); load(); })
        .catch(function(e) { status.textContent = '❌ ' + e.message; });
    });

    load();
  };

  window.router.register('/stagers', function(main) {
    window.renderStagersPage();
  });

  // ── Pivots & Port Forwards (Phase 3.1) ───────────────────
  window.renderPivotsPage = async function() {
    var main = document.getElementById('main-content');
    if (!main) return;
    var html = '<div class="panel">'
      + '<div class="panel-header"><h3>Pivots &amp; Port Forwards</h3>'
      + '<button class="btn btn-ghost btn-sm" id="pv-refresh">↻ Refresh</button>'
      + '</div>'
      + '<div class="panel-body">'
      // Pivot listener creation
      + '<form id="pv-create-form" style="display:grid;grid-template-columns:1fr 1fr 1fr 1fr;gap:6px;margin-bottom:12px;padding-bottom:12px;border-bottom:1px solid var(--border-subtle)">'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Type</label><select class="select" id="pv-type"><option value="tcp">TCP</option><option value="named-pipe">Named Pipe</option></select></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Bind Address</label><input class="input" id="pv-bind" placeholder="0.0.0.0"></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Port</label><input class="input" id="pv-port" type="number" value="8080"></div>'
      + '<button type="submit" class="btn btn-primary btn-sm" style="align-self:end">Start Pivot</button>'
      + '</form>'
      + '<h4 style="font-size:0.7rem;color:var(--text-muted);text-transform:uppercase;margin-top:12px">Pivot Graph</h4>'
      + '<div id="pv-graph" style="min-height:180px;background:var(--bg-app);padding:8px;border-radius:4px;font-family:var(--font-mono);font-size:0.85rem"></div>'
      + '<h4 style="font-size:0.7rem;color:var(--text-muted);text-transform:uppercase;margin-top:18px">Active Pivot Listeners</h4>'
      + '<div id="pv-list"></div>'
      // Port forward creation
      + '<h4 style="font-size:0.7rem;color:var(--text-muted);text-transform:uppercase;margin-top:18px">Port Forward</h4>'
      + '<form id="pv-fwd-form" style="display:grid;grid-template-columns:1fr 1fr 1fr 1fr;gap:6px;margin-bottom:12px">'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Session ID</label><input class="input" id="fwd-sid" placeholder="agent-session-id"></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Remote Host:Port</label><input class="input" id="fwd-host" placeholder="127.0.0.1:445"></div>'
      + '<div><label style="font-size:0.7rem;color:var(--text-muted)">Bind Addr</label><input class="input" id="fwd-bind" placeholder="0.0.0.0"></div>'
      + '<button type="submit" class="btn btn-primary btn-sm" style="align-self:end">Forward</button>'
      + '</form>'
      + '</div></div>';
    main.innerHTML = html;

    function renderGraph(nodes) {
      var g = document.getElementById('pv-graph');
      if (!nodes.length) {
        g.innerHTML = '<div style="color:var(--text-muted);text-align:center;padding:30px">No active pivots.</div>';
        return;
      }
      var flat = [];
      function walk(n, parent) {
        flat.push({ peer: n.peer_id, name: n.name, host: n.hostname, parent: parent, dead: n.is_dead });
        n.children.forEach(function(c) { walk(c, n.peer_id); });
      }
      nodes.forEach(function(n) { walk(n, null); });
      var lines = flat.map(function(e) {
        var prefix = e.parent != null ? '└─ ' : '● ';
        var color = e.dead ? 'var(--text-dim)' : 'var(--cyan)';
        var safe = function(s) { return String(s || '').replace(/&/g, '&amp;').replace(/</g, '&lt;'); };
        return '<div style="color:' + color + '">' + prefix + '<strong>[' + safe(e.peer) + ']</strong> ' + safe(e.name) + ' <span style="color:var(--text-muted)">@ ' + safe(e.host) + '</span></div>';
      }).join('');
      g.innerHTML = '<div style="font-family:var(--font-mono);font-size:0.8rem;line-height:1.7">' + lines + '</div>';
    }

    function renderList(items) {
      var list = document.getElementById('pv-list');
      if (!items.length) {
        list.innerHTML = '<div style="color:var(--text-muted);font-size:0.8rem;padding:8px 0">No active pivot listeners.</div>';
        return;
      }
      var rows = items.map(function(p) {
        return [
          '<strong>' + escapeHtml(p.bind_address) + '</strong>',
          escapeHtml(p.protocol),
          '<button class="btn btn-danger btn-sm" data-stop="' + p.id + '">Stop</button>',
        ];
      });
      list.innerHTML = window.renderTable(['Bind','Proto','Action'], rows);
      list.querySelectorAll('button[data-stop]').forEach(function(b) {
        b.addEventListener('click', async function() {
          try {
            await apiPost('/api/pivots/stop', { id: parseInt(b.dataset.stop) });
            showToast('Pivot stopped', 'success');
            loadAll();
          } catch(e) { showToast(e.message, 'error'); }
        });
      });
    }

    function loadAll() {
      apiGet('/api/pivots/graph').then(renderGraph).catch(function(e) {
        document.getElementById('pv-graph').innerHTML = '<div style="color:var(--red)">' + escapeHtml(e.message) + '</div>';
      });
      apiGet('/api/pivots').then(renderList).catch(function() {});
    }

    document.getElementById('pv-refresh').addEventListener('click', loadAll);

    document.getElementById('pv-create-form').addEventListener('submit', function(e) {
      e.preventDefault();
      var type = document.getElementById('pv-type').value;
      var bind = document.getElementById('pv-bind').value;
      var port = parseInt(document.getElementById('pv-port').value || '8080');
      apiPost('/api/pivots/start', { type: type, bind_address: bind || '0.0.0.0' })
        .then(function(r) { showToast(r.message, 'success'); loadAll(); })
        .catch(function(e) { showToast(e.message, 'error'); });
    });

    document.getElementById('pv-fwd-form').addEventListener('submit', function(e) {
      e.preventDefault();
      var sid = document.getElementById('fwd-sid').value.trim();
      var hostport = document.getElementById('fwd-host').value.trim();
      var bind = document.getElementById('fwd-bind').value.trim();
      if (!sid || !hostport) { showToast('Session ID and host:port required', 'error'); return; }
      var parts = hostport.split(':');
      var host = parts[0];
      var port = parseInt(parts[1] || '0');
      apiPost('/api/agents/' + encodeURIComponent(sid) + '/portfwd', {
        session_id: sid, remote_address: host, remote_port: port, bind_address: bind || undefined,
      })
        .then(function(r) { showToast(r.message, 'success'); })
        .catch(function(e) { showToast(e.message, 'error'); });
    });

    loadAll();
  };

  window.router.register('/pivots', function(main) {
    window.renderPivotsPage();
  });

  // ── Reports & Export (Phase 3.2) ────────────────────────
  window.renderReportsPage = function() {
    var main = document.getElementById('main-content');
    if (!main) return;
    var html = '<div class="panel"><div class="panel-header"><h3>Reports &amp; Export</h3>'
      + '<div style="display:flex;gap:6px;align-items:center">'
      + '<input class="input" id="rep-since" placeholder="since (ISO)" style="font-size:0.8rem">'
      + '<input class="input" id="rep-until" placeholder="until (ISO)" style="font-size:0.8rem">'
      + '<input class="input" id="rep-limit" type="number" value="500" placeholder="limit" style="width:80px;font-size:0.8rem">'
      + '</div></div>'
      + '<div class="panel-body">'
      // Sessions card
      + '<div style="display:grid;grid-template-columns:repeat(2,1fr);gap:12px">'
      + '<div class="panel"><div class="panel-header"><h3 style="font-size:0.85rem;text-transform:uppercase">Sessions</h3></div>'
      + '<div class="panel-body" id="rep-sessions-body" style="font-size:0.85rem">Loading…</div></div>'
      + '<div class="panel"><div class="panel-header"><h3 style="font-size:0.85rem;text-transform:uppercase">Credentials</h3></div>'
      + '<div class="panel-body" id="rep-creds-body" style="font-size:0.85rem">Loading…</div></div>'
      + '<div class="panel"><div class="panel-header"><h3 style="font-size:0.85rem;text-transform:uppercase">Hosts</h3></div>'
      + '<div class="panel-body" id="rep-hosts-body" style="font-size:0.85rem">Loading…</div></div>'
      + '<div class="panel"><div class="panel-header"><h3 style="font-size:0.85rem;text-transform:uppercase">Activity Timeline</h3></div>'
      + '<div class="panel-body" id="rep-timeline-body" style="font-size:0.85rem">Loading…</div></div>'
      + '</div>'
      + '<div style="margin-top:12px;display:flex;gap:8px">'
      + '<button class="btn btn-primary btn-sm" id="rep-generate">Generate Reports</button>'
      + '<button class="btn btn-ghost btn-sm" id="rep-csv">Export All (CSV)</button>'
      + '</div>'
      + '</div></div>';
    main.innerHTML = html;

    function buildQuery() {
      return '?since=' + encodeURIComponent(document.getElementById('rep-since').value || '1970-01-01')
        + '&until=' + encodeURIComponent(document.getElementById('rep-until').value || '9999-12-31')
        + '&limit=' + encodeURIComponent(document.getElementById('rep-limit').value || '500');
    }

    function renderRows(target, rows, cols) {
      var body = document.getElementById(target);
      if (!rows || !rows.length) {
        body.innerHTML = '<div style="color:var(--text-muted);padding:8px">No data.</div>';
        return;
      }
      var trs = rows.map(function(r) {
        var cells = cols.map(function(c) { return '<td>' + escapeHtml(String(r[c] === undefined || r[c] === null ? '—' : r[c])) + '</td>'; });
        return '<tr>' + cells.join('') + '</tr>';
      });
      var hs = cols.map(function(c) { return '<th>' + c + '</th>'; });
      body.innerHTML = '<table class="data-table"><thead><tr>' + hs.join('') + '</tr></thead><tbody>' + trs.join('') + '</tbody></table>';
    }

    function loadAll() {
      var q = buildQuery();
      apiGet('/api/reports/sessions' + q).then(function(rows) {
        renderRows('rep-sessions-body', rows, ['id', 'hostname', 'username', 'transport', 'is_dead']);
      }).catch(function(e) {
        document.getElementById('rep-sessions-body').innerHTML = '<div style="color:var(--red)">' + escapeHtml(e.message) + '</div>';
      });
      apiGet('/api/reports/credentials' + q).then(function(rows) {
        renderRows('rep-creds-body', rows, ['username', 'domain', 'host', 'is_cracked', 'hash_type']);
      }).catch(function(e) {
        document.getElementById('rep-creds-body').innerHTML = '<div style="color:var(--red)">' + escapeHtml(e.message) + '</div>';
      });
      apiGet('/api/reports/hosts' + q).then(function(rows) {
        renderRows('rep-hosts-body', rows, ['hostname', 'agent_count', 'last_seen']);
      }).catch(function(e) {
        document.getElementById('rep-hosts-body').innerHTML = '<div style="color:var(--red)">' + escapeHtml(e.message) + '</div>';
      });
      apiGet('/api/reports/timeline' + q).then(function(rows) {
        renderRows('rep-timeline-body', rows, ['created_at', 'action', 'target_type', 'result_status']);
      }).catch(function(e) {
        document.getElementById('rep-timeline-body').innerHTML = '<div style="color:var(--red)">' + escapeHtml(e.message) + '</div>';
      });
    }

    document.getElementById('rep-generate').addEventListener('click', loadAll);
    document.getElementById('rep-csv').addEventListener('click', function() {
      ['sessions', 'credentials', 'hosts', 'timeline'].forEach(function(name) {
        var url = '/api/reports/' + name + buildQuery() + '&format=csv';
        var link = document.createElement('a');
        link.href = url;
        link.download = name + '.csv';
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
      });
    });

    loadAll();
  };

  window.router.register('/reports', function(main) {
    window.renderReportsPage();
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
