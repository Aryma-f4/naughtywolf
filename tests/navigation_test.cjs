const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');

// Run the shipped script with only the browser/network boundary simulated.
function harness() {
  const clicks = {}, events = {}, requests = [], pushes = [], animations = [], callbackInits = [];
  const elements = new Map();
  let main;
  class Element {
    constructor(tag, attrs = {}) { this.tagName = tag; this.attrs = attrs; this.children = []; this.style = {}; this.dataset = {}; }
    getAttribute(key) { return this.attrs[key] ?? null; }
    hasAttribute(key) { return key in this.attrs; }
    setAttribute(key, value) { this.attrs[key] = value; }
    removeAttribute(key) { delete this.attrs[key]; }
    addEventListener() {}
    append(...children) { this.children.push(...children); }
    appendChild(child) { this.append(child); return child; }
    remove() { elements.delete(this.id); }
    querySelector() { return new Element('button'); }
    scrollIntoView() {}
    focus() { this.focused = true; }
    replaceWith(next) { main = next; }
    closest(selector) { return selector === 'a' ? this : null; }
  }
  main = new Element('main');
  const navLinks = ['/dashboard', '/operations', '/inventory'].map(href => new Element('a', { href }));
  const location = { href: 'http://localhost/dashboard', origin: 'http://localhost', pathname: '/dashboard', search: '', hash: '' };
  const document = {
    title: 'Dashboard', body: new Element('body'),
    querySelector: selector => selector === 'main' || selector === '.portal-main' ? main : selector === '.page-heading' ? selector : null,
    querySelectorAll: selector => selector === '.nav-link, .quick-link' ? navLinks : [],
    getElementById: id => elements.get(id) || null,
    createElement: tag => { const el = new Element(tag); Object.defineProperty(el, 'id', { get() { return this._id; }, set(id) { this._id = id; elements.set(id, this); } }); return el; },
    addEventListener: (type, fn) => { clicks[type] = fn; },
  };
  const anime = options => animations.push(options.targets);
  anime.stagger = () => 0;
  const window = {
    location, scrollTo: () => { window.scrolls++; }, scrolls: 0,
    matchMedia: () => ({ matches: false }),
    addEventListener: (type, fn) => { events[type] = fn; },
    NWCallbackWorkspace: { init: root => callbackInits.push(root) },
  };
  const context = {
    document, window, location, URL, AbortController, anime,
    history: { pushState: (_, __, href) => { pushes.push(href); } },
    fetch: (href, options) => new Promise((resolve, reject) => requests.push({ href, options, resolve, reject })),
    DOMParser: class { parseFromString(doc) { return doc; } },
    setInterval: () => 1, clearInterval() {}, setTimeout, clearTimeout,
  };
  vm.runInNewContext(fs.readFileSync('static/admin.js', 'utf8'), context);
  animations.length = 0;
  const flush = () => new Promise(resolve => setImmediate(resolve));
  return {
    requests, pushes, navLinks, animations, elements, window, events, callbackInits, get main() { return main; },
    click(href, attrs = {}) {
      const e = { button: 0, target: new Element('a', { href, ...attrs }), preventDefault() { this.prevented = true; } };
      clicks.click(e); return e;
    },
    async reply(index, title, active = '/operations') {
      const next = new Element('main'); next.title = title;
      const doc = { querySelector: selector => selector === 'main' ? next : selector === 'title' ? { textContent: title } : null,
        querySelectorAll: () => [new Element('a', { href: active })] };
      requests[index].resolve({ ok: true, status: 200, url: `http://localhost${requests[index].href}`, headers: { get: () => 'text/html' }, text: async () => doc });
      await flush();
    }, flush,
  };
}

test('page initialization delegates callback task state to the persistent workspace client', async () => {
  const h = harness();
  assert.equal(h.callbackInits.length, 1);
  assert.equal(h.callbackInits[0], h.main);
  h.click('/operations');
  await h.reply(0, 'Operations');
  assert.equal(h.callbackInits.length, 2);
  assert.equal(h.callbackInits[1], h.main);
});

test('Back navigation does not create another history entry', async () => {
  const h = harness(); h.window.location.pathname = '/operations';
  h.events.popstate(); await h.reply(0, 'Operations');
  assert.equal(h.pushes.length, 0);
});

test('the most recent menu click wins even if an older request finishes last', async () => {
  const h = harness(); h.click('/operations'); h.click('/inventory');
  await h.reply(1, 'Assets', '/inventory'); await h.reply(0, 'Operations');
  assert.equal(h.main.title, 'Assets');
  assert.deepEqual(h.pushes, ['/inventory']);
});

test('subpages retain the active navigation supplied by the server', async () => {
  const h = harness(); h.click('/operations/new'); await h.reply(0, 'Create operation');
  assert.equal(h.navLinks[1].getAttribute('aria-current'), 'page');
});

test('network failure keeps the current page and exposes a retry link', async () => {
  const h = harness(); const original = h.main;
  h.click('/operations'); h.requests[0].reject(new Error('offline')); await h.flush();
  assert.equal(h.main, original); assert.equal(h.pushes.length, 0);
  const notice = h.elements.get('navigation-error');
  assert.equal(notice?.getAttribute('role'), 'alert');
  assert(notice.children.some(child => child.href === '/operations'));
});

test('successful navigation focuses the new content without animating navigation', async () => {
  const h = harness(); h.click('/operations'); await h.reply(0, 'Operations');
  assert.equal(h.main.focused, true);
  assert(h.animations.every(target => typeof target === 'string' && (target.startsWith('.portal-main') || target === '.page-heading')));
});

for (const [href, attrs] of [
  ['/evidence/item/download', {}], ['/reports', { download: '' }],
  ['/operations', { target: '_blank' }], ['//example.com/report', {}],
  ['/payloads/download/test', {}], ['#main-content', {}],
]) {
  test(`browser handles ${href} ${JSON.stringify(attrs)} normally`, () => {
    const h = harness(); const event = h.click(href, attrs);
    assert.equal(event.prevented, undefined); assert.equal(h.requests.length, 0);
  });
}

test('pending navigation announces busy state and clears it after success', async () => {
  const h = harness(); h.click('/operations');
  assert.equal(h.main.getAttribute('aria-busy'), 'true');
  await h.reply(0, 'Operations');
  assert.equal(h.main.hasAttribute('aria-busy'), false);
});

test('a superseded request failure cannot obscure the latest page', async () => {
  const h = harness(); h.click('/operations'); h.click('/inventory');
  assert.equal(h.requests[0].options.signal.aborted, true);
  await h.reply(1, 'Assets', '/inventory');
  h.requests[0].reject(new Error('aborted')); await h.flush();
  assert.equal(h.elements.has('navigation-error'), false);
  assert.equal(h.main.title, 'Assets');
});

test('clicking the current menu does not duplicate the history entry', async () => {
  const h = harness(); h.click('/dashboard'); await h.reply(0, 'Dashboard', '/dashboard');
  assert.equal(h.pushes.length, 0);
});

test('server failure preserves the content and clears busy state', async () => {
  const h = harness(); const original = h.main; h.click('/operations');
  h.requests[0].resolve({ ok: false, status: 503, url: 'http://localhost/operations' });
  await h.flush();
  assert.equal(h.main, original);
  assert.equal(h.main.hasAttribute('aria-busy'), false);
  assert.equal(h.elements.get('navigation-error')?.getAttribute('role'), 'alert');
});

test('expired authentication returns to sign-in', async () => {
  const h = harness(); h.click('/operations');
  h.requests[0].resolve({ ok: false, status: 401, url: 'http://localhost/operations' });
  await h.flush();
  assert.equal(h.window.location.href, '/login');
  assert.equal(h.pushes.length, 0);
});
