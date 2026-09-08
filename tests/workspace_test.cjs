const { test } = require('node:test');
const assert = require('node:assert/strict');
const { filterGraph, visibleGraph, layoutGraph, sampleGraph, MAX_VISIBLE } = require('../static/workspace.js');

test('search retains the actual operation and asset context of a callback', () => {
  const data = sampleGraph();
  const result = filterGraph(data, { query: 'LAB-101' });
  assert.equal(result.nodes.length, 3);
  assert.equal(result.edges.length, 2);
  assert.deepEqual(result.nodes.filter(n => !n.contextual).map(n => n.label), ['LAB-101']);
  assert.equal(filterGraph(data, { query: 'missing-host' }).nodes.length, 0);
});

test('operation and status filters compose without introducing unrelated targets', () => {
  const result = filterGraph(sampleGraph(), { operation: 'sample-op-1', status: 'beacon' });
  assert.equal(result.nodes.length, 3);
  assert.ok(result.nodes.every(n => n.operation === 'sample-op-1'));
  assert.equal(result.nodes.filter(n => !n.contextual).length, 1);
});

test('large inventories stay bounded while preserving every visible parent chain', () => {
  const root = { id: 'op', kind: 'operation', facts: [] };
  const data = { nodes: [root], edges: [] };
  for (let i = 0; i < 1000; i++) {
    data.nodes.push({ id: `a${i}`, kind: 'asset', facts: [] }, { id: `c${i}`, kind: 'callback', facts: [] });
    data.edges.push({ source: 'op', target: `a${i}` }, { source: `a${i}`, target: `c${i}` });
  }
  const visible = visibleGraph(data);
  const ids = new Set(visible.nodes.map(n => n.id));
  assert.equal(visible.nodes.length, MAX_VISIBLE);
  assert.ok(visible.edges.every(e => ids.has(e.source) && ids.has(e.target)));
  assert.ok(visible.nodes.filter(n => n.kind === 'callback').every(n => ids.has(n.id.replace('c', 'a'))));
  assert.ok(ids.has('op'));
  assert.equal(data.nodes.length, 2001);
});

test('layout provides non-overlapping sibling cards and positions unlinked nodes', () => {
  const data = sampleGraph();
  data.nodes.push({ id: 'unassigned', kind: 'callback', facts: [] });
  const positions = layoutGraph(data);
  assert.equal(positions.size, data.nodes.length);
  for (const [id, p] of positions) {
    assert.ok(Number.isFinite(p.x) && Number.isFinite(p.y));
    for (const [otherId, q] of positions) {
      if (id !== otherId && p.x === q.x) assert.ok(Math.abs(p.y - q.y) >= 63);
    }
  }
});

test('malformed cyclic relationships do not hang filtering or layout', () => {
  const data = { nodes: ['a', 'b'].map(id => ({ id, kind: 'asset', facts: [], label: id })), edges: [{ source: 'a', target: 'b' }, { source: 'b', target: 'a' }] };
  assert.equal(filterGraph(data, { query: 'a' }).nodes.length, 2);
  assert.equal(layoutGraph(data).size, 2);
});

test('sample data is isolated and contains no operational action links', () => {
  const data = sampleGraph();
  assert.equal(data.nodes.length, 14);
  assert.ok(data.nodes.every(n => n.id.startsWith('sample-') && n.href === ''));
  assert.ok(data.nodes.filter(n => n.kind === 'asset').every(n => n.detail.startsWith('192.0.2.')));
  assert.equal(sampleGraph().nodes.length, 14);
});
