/* Shared pure graph model; also exercised by the dependency-free Node tests. */
((root) => {
  const MAX_VISIBLE = 240;
  function filterGraph(data, { query = '', operation = '', status = '' } = {}) {
    const needle = query.trim().toLowerCase();
    const lookup = new Map(data.nodes.map(n => [n.id, n]));
    const parents = new Map(data.edges.map(e => [e.target, e.source]));
    const matches = data.nodes.filter(n => (!operation || n.operation === operation) && (!status || n.status === status)
      && (!needle || [n.label, n.detail, n.kind, ...n.facts.flat()].join(' ').toLowerCase().includes(needle)));
    const included = new Set();
    // Keep the context chain for each match, even when the parent doesn't match a status filter.
    for (const n of matches) {
      let id = n.id;
      const visited = new Set();
      while (lookup.has(id) && !visited.has(id)) { included.add(id); visited.add(id); id = parents.get(id); }
    }
    const matched = new Set(matches.map(n => n.id));
    return { nodes: data.nodes.filter(n => included.has(n.id)).map(n => ({ ...n, contextual: !matched.has(n.id) })), edges: data.edges.filter(e => included.has(e.source) && included.has(e.target)) };
  }
  function visibleGraph(data) {
    if (data.nodes.length <= MAX_VISIBLE) return data;
    const lookup = new Map(data.nodes.map(n => [n.id, n]));
    const parents = new Map(data.edges.map(e => [e.target, e.source]));
    const kept = new Set();
    const priority = { callback: 0, asset: 1, address: 2, operation: 3 };
    for (const node of [...data.nodes].sort((a, b) => priority[a.kind] - priority[b.kind])) {
      const chain = new Set(); let id = node.id;
      while (lookup.has(id) && !chain.has(id) && !kept.has(id)) { chain.add(id); id = parents.get(id); }
      if (kept.size + chain.size <= MAX_VISIBLE) for (const item of chain) kept.add(item);
    }
    return { nodes: data.nodes.filter(n => kept.has(n.id)), edges: data.edges.filter(e => kept.has(e.source) && kept.has(e.target)) };
  }
  function layoutGraph(data) {
    const lookup = new Map(data.nodes.map(n => [n.id, n]));
    const children = new Map(); const hasParent = new Set();
    for (const edge of data.edges) {
      if (!lookup.has(edge.source) || !lookup.has(edge.target)) continue;
      if (!children.has(edge.source)) children.set(edge.source, []);
      children.get(edge.source).push(edge.target); hasParent.add(edge.target);
    }
    let row = 0; const positions = new Map(); const visiting = new Set();
    function visit(id, depth) {
      if (positions.has(id) || visiting.has(id)) return;
      visiting.add(id);
      const start = row;
      for (const child of children.get(id) || []) visit(child, depth + 1);
      if (row === start) row++;
      positions.set(id, { x: 50 + depth * 280, y: 40 + (start + row - 1) * 46 });
      visiting.delete(id);
    }
    for (const n of data.nodes.filter(n => !hasParent.has(n.id))) { visit(n.id, 0); row += .45; }
    for (const n of data.nodes) if (!positions.has(n.id)) visit(n.id, 0);
    return positions;
  }
  function sampleGraph() {
    const nodes = [], edges = [];
    for (let group = 0; group < 2; group++) {
      const op = `sample-op-${group}`;
      nodes.push({ id: op, kind: 'operation', label: ['Northstar lab', 'Redwood lab'][group], detail: 'Illustrative scope', operation: op, status: 'active', href: '', facts: [] });
      for (let host = 0; host < 3; host++) {
        const asset = `${op}-asset-${host}`;
        const label = ['Gateway', 'Workstation', 'Application'][host] + ` ${group + 1}`;
        nodes.push({ id: asset, kind: 'asset', label, detail: `192.0.2.${group * 10 + host + 10}`, operation: op, status: 'active', href: '', facts: [['Dataset', 'Sample only'], ['Owner', 'Lab team']] });
        edges.push({ source: op, target: asset, label: 'contains' });
        const id = `${asset}-callback`;
        nodes.push({ id, kind: 'callback', label: `LAB-${group + 1}0${host + 1}`, detail: ['Windows · HTTPS', 'Linux · HTTP', 'macOS · HTTPS'][host], operation: op, status: ['active', 'beacon', 'dormant'][host], href: '', facts: [['Dataset', 'Sample only'], ['Relationship', 'Illustrative registered callback']] });
        edges.push({ source: asset, target: id, label: 'registered callback' });
      }
    }
    return { nodes, edges };
  }
  const model = { filterGraph, visibleGraph, layoutGraph, sampleGraph, MAX_VISIBLE };
  if (typeof module !== 'undefined' && module.exports) { module.exports = model; return; }
  root.NWGraph = model;
  const svgNS = 'http://www.w3.org/2000/svg';
  const svgEl = (name, attrs = {}, text = '') => {
    const el = document.createElementNS(svgNS, name);
    for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
    if (text) el.textContent = text;
    return el;
  };
  const el = (name, className = '', text = '') => { const node = document.createElement(name); node.className = className; node.textContent = text; return node; };
  const trim = (text, limit) => text.length > limit ? text.slice(0, limit - 1) + '…' : text;
  let cleanup = () => {};

  function init(main) {
    cleanup();
    if (!main) return;
    const lifetime = new AbortController(); const { signal } = lifetime;
    const cleanups = [];
    cleanup = () => { lifetime.abort(); cleanups.forEach(fn => fn()); };
    const on = (node, type, fn) => node?.addEventListener(type, fn, { signal });
    const scope = main.querySelector('[data-topology]');
    if (scope) initGraph(scope, on, signal, cleanups);
    const historyFilter = main.querySelector('.recon-history-filter');
    on(historyFilter, 'submit', event => {
      event.preventDefault();
      const query = new URLSearchParams(new FormData(historyFilter));
      root.NW.navigate(`/recon?${query}`);
    });
    const form = main.querySelector('[data-recon-form]');
    on(form, 'submit', async event => {
      event.preventDefault();
      const button = form.querySelector('button[type="submit"]');
      const progress = form.querySelector('[data-recon-progress]');
      const payload = new URLSearchParams(new FormData(form));
      button.disabled = true; button.textContent = 'Observing target…';
      progress.textContent = 'Resolving addresses and collecting the selected observations. This can take up to 15 seconds.';
      try {
        const response = await fetch(form.action, { method: 'POST', body: payload, credentials: 'same-origin' });
        if (new URL(response.url).pathname === '/login') { window.location.href = '/login'; return; }
        if (!response.ok) {
          const doc = new DOMParser().parseFromString(await response.text(), 'text/html');
          const message = doc.querySelector('.form-error')?.textContent;
          throw new Error(message || (response.status === 409 ? 'Check that the asset and operation are active, or try again when other checks finish.' : 'The request was not accepted. Check your scope and refresh the page before retrying.'));
        }
        if (!signal.aborted) await root.NW.navigate(new URL(response.url).pathname + new URL(response.url).search);
      } catch (error) {
        if (!signal.aborted) progress.textContent = error instanceof TypeError ? 'Could not confirm completion. Refresh history before trying again.' : error.message;
      } finally { button.disabled = false; button.textContent = 'Run reconnaissance ↗'; }
    });
  }

  function initGraph(scope, on, signal, cleanups) {
    const $ = selector => scope.querySelector(selector);
    const canvas = $('[data-graph-canvas]'), svg = $('[data-graph-svg]'), layer = $('[data-graph-layer]');
    const list = $('[data-graph-list]'), detail = $('[data-node-detail]');
    const operation = $('[data-graph-operation]'), search = $('[data-graph-search]'), status = $('[data-graph-status]');
    let live = { nodes: [], edges: [] }, data = live, graph = live, positions = new Map();
    let sample = false, selected = '', view = 'graph', loading = false;
    let camera = { x: 20, y: 20, k: 1 }, drag = null, bounds = { width: 800, height: 600 };
    const size = () => ({ width: Math.max(canvas.clientWidth, 280), height: Math.max(canvas.clientHeight, 400) });
    function move() { layer.setAttribute('transform', `translate(${camera.x} ${camera.y}) scale(${camera.k})`); }
    function fit() {
      const { width, height } = size(); svg.setAttribute('viewBox', `0 0 ${width} ${height}`);
      camera.k = Math.max(.08, Math.min(1.2, (width - 50) / bounds.width, (height - 65) / bounds.height));
      camera.x = (width - bounds.width * camera.k) / 2; camera.y = (height - bounds.height * camera.k) / 2;
      move();
    }
    function zoom(factor) {
      const { width, height } = size(); const next = Math.max(.08, Math.min(3, camera.k * factor));
      camera.x = width / 2 - (width / 2 - camera.x) * next / camera.k;
      camera.y = height / 2 - (height / 2 - camera.y) * next / camera.k;
      camera.k = next; move();
    }
    function select(id) {
      const changed = selected !== id;
      selected = id;
      const node = data.nodes.find(n => n.id === id); if (!node) return;
      scope.querySelectorAll('[data-node-id]').forEach(g => {
        const active = g.getAttribute('data-node-id') === id;
        g.classList.toggle('selected', active);
        g.setAttribute('aria-pressed', String(active));
      });
      layer.querySelectorAll('[data-edge]').forEach(g => g.classList.toggle('selected', g.getAttribute('data-source') === id || g.getAttribute('data-target') === id));
      root.NWMotion?.clearGraph(detail);
      detail.replaceChildren();
      detail.append(el('span', `inspector-kind kind-${node.kind}`, node.kind), el('h2', '', node.label), el('p', 'inspector-address', node.detail));
      const facts = el('dl', 'node-facts');
      for (const [name, value] of [['Status', node.status], ...node.facts]) { const row = el('div'); row.append(el('dt', '', name), el('dd', '', value || '—')); facts.append(row); }
      detail.append(facts);
      if (node.href && !sample) { const link = el('a', 'button', node.kind === 'callback' ? 'Open callback ↗' : node.kind === 'operation' ? 'Review operation ↗' : 'Open recon history ↗'); link.href = node.href; detail.append(link); }
      if (node.operation) { const focus = el('button', 'btn btn-ghost', 'Focus this operation'); focus.addEventListener('click', () => { operation.value = node.operation; render(true); }); detail.append(focus); }
      if (sample) detail.append(el('p', 'field-hint', 'Sample node. No target or callback was created.'));
      if (changed) root.NWMotion?.detail(detail);
    }
    function resetInspector() {
      root.NWMotion?.clearGraph(detail);
      selected = ''; detail.replaceChildren(el('span', 'inspector-symbol', '⌖'), el('h2', '', 'Every connection has context.'), el('p', '', 'Select a node to inspect its recorded identity and relationships.'));
    }
    function setData(next, reveal = true) {
      data = next; operation.replaceChildren(new Option('All operations', ''));
      for (const node of data.nodes.filter(n => n.kind === 'operation')) operation.append(new Option(node.label, node.id.replace(/^operation:/, '')));
      search.value = ''; status.value = ''; resetInspector(); render(true, reveal);
    }
    function render(reset = false, reveal = false) {
      const filtered = filterGraph(data, { query: search.value, operation: operation.value, status: status.value });
      graph = visibleGraph(filtered); positions = layoutGraph(graph);
      root.NWMotion?.clearGraph(layer);
      root.NWMotion?.clearGraph(detail);
      layer.replaceChildren(); list.replaceChildren();
      bounds = { width: 1, height: 1 };
      for (const p of positions.values()) { bounds.width = Math.max(bounds.width, p.x + 225); bounds.height = Math.max(bounds.height, p.y + 80); }
      $('[data-graph-empty]').hidden = data.nodes.length > 0;
      $('[data-sample-banner]').hidden = !sample;
      for (const kind of ['operation', 'asset', 'callback', 'address']) $(`[data-count="${kind}"]`).textContent = data.nodes.filter(n => n.kind === kind).length;
      const note = $('[data-graph-visible]');
      note.textContent = filtered.nodes.length > MAX_VISIBLE ? `${graph.nodes.length} of ${filtered.nodes.length} nodes · refine search or operation` : `${graph.nodes.length} nodes · ${graph.edges.length} relationships`;
      if (!filtered.nodes.length && data.nodes.length) note.textContent = 'No matches. Clear the search or filters.';
      for (const edge of graph.edges) {
        const a = positions.get(edge.source), b = positions.get(edge.target); if (!a || !b) continue;
        const x = a.x + 210, y = a.y + 31, end = b.x, endY = b.y + 31;
        const path = svgEl('path', { d: `M${x},${y} C${x + 35},${y} ${end - 35},${endY} ${end},${endY}`, class: 'graph-edge', 'data-edge': '', 'data-source': edge.source, 'data-target': edge.target });
        path.append(svgEl('title', {}, edge.label)); layer.append(path);
      }
      for (const node of graph.nodes) {
        const p = positions.get(node.id);
        const g = svgEl('g', { transform: `translate(${p.x} ${p.y})`, class: `graph-node kind-${node.kind}${node.contextual ? ' contextual' : ''}`, 'data-node-id': node.id, tabindex: '0', role: 'button', 'aria-label': `${node.kind}: ${node.label}, ${node.status}` });
        g.append(svgEl('title', {}, `${node.label}\n${node.detail}`), svgEl('rect', { width: 210, height: 63, rx: 9, class: 'node-card' }), svgEl('rect', { x: 12, y: 14, width: 33, height: 33, rx: 7, class: 'node-icon-box' }));
        const symbols = { operation: '◇', asset: '▣', callback: 'ϟ', address: '◎' };
        g.append(svgEl('text', { x: 28, y: 37, class: 'node-symbol', 'text-anchor': 'middle' }, symbols[node.kind]), svgEl('text', { x: 55, y: 27, class: 'node-title' }, trim(node.label, 20)), svgEl('text', { x: 55, y: 45, class: 'node-subtitle' }, trim(node.detail || node.kind, 24)), svgEl('circle', { cx: 198, cy: 13, r: 3, class: `node-status status-${node.status}` }));
        g.addEventListener('click', () => select(node.id));
        g.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); select(node.id); } });
        layer.append(g);
        const row = el('button', `graph-list-row kind-${node.kind}`); row.type = 'button';
        row.setAttribute('data-node-id', node.id);
        row.append(el('span', 'list-kind', node.kind), el('strong', '', node.label), el('span', 'list-detail', node.detail), el('span', 'list-status', node.status));
        row.addEventListener('click', () => select(node.id)); list.append(row);
      }
      if (selected && graph.nodes.some(n => n.id === selected)) select(selected); else if (selected) resetInspector();
      if (reset) fit(); else move();
      if (reveal) root.NWMotion?.graph(layer);
    }
    async function refresh() {
      if (loading) return; loading = true; $('[data-graph-refresh]').disabled = true;
      try {
        const response = await fetch('/topology/data', { credentials: 'same-origin', signal });
        if (!response.ok) throw new Error('Could not load the workspace. Use Refresh to retry.');
        const next = await response.json(); if (signal.aborted) return;
        live = next;
        if (!sample) {
          const selection = { operation: operation.value, query: search.value, status: status.value, selected };
          setData(live, false); operation.value = [...operation.options].some(o => o.value === selection.operation) ? selection.operation : '';
          search.value = selection.query; status.value = selection.status; selected = selection.selected; render(true, true);
        }
        $('[data-graph-snapshot]').textContent = `Snapshot · ${new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
      } catch (error) { if (!signal.aborted) $('[data-graph-snapshot]').textContent = 'Could not load snapshot. Refresh to retry.'; }
      finally { loading = false; $('[data-graph-refresh]').disabled = false; }
    }
    on(search, 'input', () => render(true)); on(operation, 'change', () => render(true)); on(status, 'change', () => render(true));
    on($('[data-graph-refresh]'), 'click', refresh);
    on($('[data-graph-demo]'), 'click', () => { sample = true; setData(sampleGraph()); });
    on($('[data-graph-live]'), 'click', () => { sample = false; setData(live); });
    for (const button of scope.querySelectorAll('[data-view]')) on(button, 'click', () => {
      view = button.dataset.view; canvas.hidden = view !== 'graph'; list.hidden = view !== 'list';
      scope.querySelectorAll('[data-zoom]').forEach(control => { control.hidden = view !== 'graph'; });
      scope.querySelectorAll('[data-view]').forEach(b => b.setAttribute('aria-pressed', String(b === button)));
      if (view === 'graph') fit();
    });
    for (const button of scope.querySelectorAll('[data-zoom]')) on(button, 'click', () => button.dataset.zoom === 'fit' ? fit() : zoom(button.dataset.zoom === 'in' ? 1.25 : .8));
    on(canvas, 'pointerdown', event => { if (event.target.closest('[data-node-id]') || event.button !== 0) return; canvas.setPointerCapture(event.pointerId); drag = { x: event.clientX, y: event.clientY, cx: camera.x, cy: camera.y }; });
    on(canvas, 'pointermove', event => { if (drag) { camera.x = drag.cx + event.clientX - drag.x; camera.y = drag.cy + event.clientY - drag.y; move(); } });
    on(canvas, 'pointerup', () => { drag = null; }); on(canvas, 'pointercancel', () => { drag = null; });
    on(canvas, 'keydown', event => {
      if (event.target !== canvas) return;
      if (['+', '=', '-', '0', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) event.preventDefault();
      if (event.key === '+' || event.key === '=') zoom(1.25); else if (event.key === '-') zoom(.8); else if (event.key === '0') fit();
      else { const delta = { ArrowLeft: [40, 0], ArrowRight: [-40, 0], ArrowUp: [0, 40], ArrowDown: [0, -40] }[event.key]; if (delta) { camera.x += delta[0]; camera.y += delta[1]; move(); } }
    });
    const resize = new ResizeObserver(() => { if (view === 'graph') fit(); }); resize.observe(canvas); cleanups.push(() => resize.disconnect());
    refresh();
  }
  root.NWWorkspace = { init };
})(typeof window !== 'undefined' ? window : globalThis);
