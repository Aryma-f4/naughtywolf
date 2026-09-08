const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');

function harness(reduced = false) {
  const calls = [], events = {}, preference = { matches: reduced, addEventListener: (_, fn) => { preference.change = fn; } };
  const node = (page = true) => ({
    attrs: new Map([['style', 'color: red']]),
    getAttribute(key) { return this.attrs.get(key) ?? null; },
    setAttribute(key, value) { this.attrs.set(key, value); },
    removeAttribute(key) { this.attrs.delete(key); },
    closest: () => page ? main : null,
  });
  const brand = node(false), heading = node(), card = node();
  const main = { querySelectorAll: selector => selector.includes('.page-heading') ? [heading] : selector.includes('.overview-hero') ? [card] : [] };
  const document = { hidden: false, querySelector: () => null, querySelectorAll: () => [brand], addEventListener: (name, fn) => { events[name] = fn; } };
  const engine = options => {
    calls.push(options);
    for (const target of options.targets) target.setAttribute('style', 'opacity: 0; transform: translateY(16px)');
  };
  engine.remove = () => {};
  engine.stagger = () => () => 0;
  engine.setDashoffset = () => 40;
  const window = { anime: engine, matchMedia: () => preference, addEventListener: (name, fn) => { events[name] = fn; } };
  vm.runInNewContext(fs.readFileSync(require.resolve('../static/motion.js'), 'utf8'), { window, document });
  return { api: window.NWMotion, calls, events, preference, document, main, brand, heading, card, node };
}

test('menu/page entrances animate content without replaying the header brand', () => {
  const h = harness();
  assert.equal(h.calls.length, 1);
  h.api.page(h.main);
  assert.ok(h.calls.slice(1).every(call => !call.targets.includes(h.brand)));
  assert.ok(h.calls.some(call => call.targets.includes(h.card)));
  h.api.clearPage();
  assert.equal(h.card.getAttribute('style'), 'color: red');
  assert.equal(h.heading.getAttribute('style'), 'color: red');
});

test('reduced motion skips entrance, graph, detail and brand animation', () => {
  const h = harness(true);
  h.api.page(h.main);
  h.api.graph({ querySelectorAll: () => [h.card] });
  h.api.detail({ children: [h.card] });
  assert.equal(h.calls.length, 0);
  assert.equal(h.card.getAttribute('style'), 'color: red');
});

test('enabling reduced motion midway restores visible content and SVG attributes', () => {
  const h = harness(); h.api.page(h.main);
  h.brand.setAttribute('stroke-dasharray', '40');
  h.brand.setAttribute('stroke-dashoffset', '40');
  h.preference.matches = true; h.preference.change();
  for (const node of [h.brand, h.heading, h.card]) assert.equal(node.getAttribute('style'), 'color: red');
  assert.equal(h.brand.getAttribute('stroke-dasharray'), null);
  assert.equal(h.brand.getAttribute('stroke-dashoffset'), null);
});

test('hidden tabs cancel animation, and stale completion cannot overwrite a newer animation', () => {
  const h = harness(); h.api.page(h.main);
  const old = h.calls.find(call => call.targets.includes(h.card));
  h.api.detail({ children: [h.card] });
  old.complete();
  assert.match(h.card.getAttribute('style'), /opacity: 0/);
  h.document.hidden = true; h.events.visibilitychange();
  assert.equal(h.card.getAttribute('style'), 'color: red');
  const count = h.calls.length;
  h.api.page(h.main);
  assert.equal(h.calls.length, count);
});

test('dense graphs render immediately and graph cleanup preserves node placement', () => {
  const h = harness();
  const cards = Array.from({ length: 81 }, () => h.node());
  const count = h.calls.length;
  h.api.graph({ querySelectorAll: () => cards });
  assert.equal(h.calls.length, count);
  const edge = h.node(); edge.setAttribute('transform', 'translate(50 90)');
  const layer = { querySelectorAll: () => [edge], contains: target => target === edge };
  h.api.graph(layer); h.api.clearGraph(layer);
  assert.equal(edge.getAttribute('style'), 'color: red');
  assert.equal(edge.getAttribute('transform'), 'translate(50 90)');
});
