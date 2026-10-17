const test = require('node:test');
const assert = require('node:assert/strict');
const { render, keyToBytes, decodeChunk, createTerminal } = require('../static/shell-terminal.js');

// A DOM stub with only what the terminal touches. Keeping it this small means a
// change to the panel's markup fails here rather than in a browser.
class Element {
  constructor(tag = 'div') {
    this.tag = tag;
    this.className = '';
    this.children = [];
    this.dataset = {};
    this.attributes = {};
    this.hidden = false;
    this.disabled = false;
    this.value = '';
    this.textContent = '';
    this.listeners = new Map();
    this.scrollTop = 0;
    this.scrollHeight = 0;
    this.clientHeight = 100;
  }
  setAttribute(name, value) { this.attributes[name] = value; }
  getAttribute(name) { return this.attributes[name]; }
  removeAttribute(name) { delete this.attributes[name]; }
  append(...nodes) { this.children.push(...nodes); }
  appendChild(node) { this.children.push(node); return node; }
  insertAdjacentHTML(_where, html) { this.children.push({ html }); }
  removeChild(node) { this.children = this.children.filter(child => child !== node); }
  get firstElementChild() { return this.children[0]; }
  get childElementCount() { return this.children.length; }
  addEventListener(type, fn) { this.listeners.set(type, fn); }
  dispatch(type, event = {}) { return this.listeners.get(type)?.(event); }
  focus() { this.focused = true; }
}

const build = () => {
  const panel = new Element('section');
  const screen = new Element('div');
  const placeholder = new Element('p');
  const state = new Element('span');
  const message = new Element('p');
  const attach = new Element('button');
  const program = new Element('select');
  const form = new Element('form');
  const input = new Element('input');
  const send = new Element('button');
  const close = new Element('button');
  const bar = new Element('div');
  const badge = new Element('span');
  const detail = new Element('span');
  const map = {
    '[data-shell-screen]': screen,
    '[data-shell-placeholder]': placeholder,
    '[data-shell-state]': state,
    '[data-shell-message]': message,
    '[data-shell-attach]': attach,
    '[data-shell-program]': program,
    '[data-shell-form]': form,
    '[data-shell-input]': input,
    '[data-shell-send]': send,
    '[data-shell-close]': close,
    '[data-shell-session-bar]': bar,
    '[data-shell-badge]': badge,
    '[data-shell-session-detail]': detail,
  };
  panel.querySelector = selector => map[selector] || null;
  panel.dataset = {
    shellsEndpoint: '/api/callbacks/cb-1/shell',
    shellStreamEndpoint: '/api/callbacks/cb-1/shell/stream',
    shellCapable: 'true',
    csrfToken: 'csrf-token',
  };
  return { panel, screen, placeholder, state, message, attach, program, form, input, close, badge, detail };
};

test('output renders as escaped text with colour applied', () => {
  const html = render('root:\x1b[31;1mdenied\x1b[0m\n');
  assert.match(html, /root:/);
  assert.match(html, /color:#f47067/);
  assert.match(html, /font-weight:600/);
  assert.doesNotMatch(html, /\[31;1m/, 'the escape sequence itself is consumed');
});

test('html in shell output cannot inject markup', () => {
  const html = render('<img src=x onerror=alert(1)>');
  assert.match(html, /&lt;img/);
  assert.doesNotMatch(html, /<img/);
});

test('an unknown escape sequence is shown rather than swallowed', () => {
  const html = render('before\x1b[?25lafter');
  assert.match(html, /before/);
  assert.match(html, /after/);
});

test('a carriage return rewrites the line and backspace erases', () => {
  const html = render('50%\r100%\b\b\b');
  assert.match(html, /data-carriage-return/);
  assert.match(html, /data-backspace/);
});

test('keys become the bytes a terminal expects', () => {
  assert.equal(keyToBytes({ key: 'Enter' }), '\r');
  assert.equal(keyToBytes({ key: 'Backspace' }), '\x7f');
  assert.equal(keyToBytes({ key: 'Tab' }), '\t');
  assert.equal(keyToBytes({ key: 'ArrowUp' }), '\x1b[A');
  assert.equal(keyToBytes({ key: 'a' }), 'a');
  assert.equal(keyToBytes({ key: 'A', ctrlKey: true }), '\x01');
  assert.equal(keyToBytes({ key: 'c', ctrlKey: true }), '\x03');
});

test('keys the terminal does not own are not sent', () => {
  assert.equal(keyToBytes({ key: 'Shift' }), null);
  assert.equal(keyToBytes({ key: 'F5' }), null);
  assert.equal(keyToBytes({ key: 'v', metaKey: true }), null);
});

test('base64 output is decoded as utf-8', () => {
  assert.equal(decodeChunk(Buffer.from('héllo').toString('base64')), 'héllo');
  assert.equal(decodeChunk('not base64!!'), '');
});

test('attaching a shell reports whether the frame was pushed or queued', async () => {
  const ui = build();
  const sent = [];
  const request = async (url, options = {}) => {
    sent.push({ url, body: options.body ? JSON.parse(options.body) : null });
    if (url.endsWith('/shell') && options.method === 'POST') {
      return {
        ok: true,
        json: async () => ({
          shell: { id: 'shell-1', program: '', args: [], pty: true, state: 'starting', opened_at: '', exit_code: null, scrollback: '' },
          delivery: 'push',
        }),
      };
    }
    return { ok: true, json: async () => ({ delivery: 'push', shell_id: 'shell-1', bytes: 4 }) };
  };
  const terminal = createTerminal(ui.panel, { fetch: request, EventSource: null, document: null });

  await ui.attach.dispatch('click');
  assert.equal(sent[0].url, '/api/callbacks/cb-1/shell');
  assert.equal(sent[0].body.program, '', 'an empty program means the platform shell');
  assert.match(ui.message.textContent, /live channel/);
  assert.equal(ui.state.textContent, 'Attached');

  // Typing goes to the attached shell, not to a new task.
  ui.input.value = 'whoami';
  await ui.form.dispatch('submit', { preventDefault() {} });
  assert.equal(sent[1].url, '/api/callbacks/cb-1/shell/input');
  assert.equal(sent[1].body.data, 'whoami\r');
  assert.equal(ui.input.value, '');

  await terminal.sendInput('\x03');
  assert.equal(sent[2].body.data, '\x03');
});

test('input without an attached shell is refused before it reaches the network', async () => {
  const ui = build();
  let calls = 0;
  const request = async () => { calls += 1; return { ok: true, json: async () => ({}) }; };
  const terminal = createTerminal(ui.panel, { fetch: request, EventSource: null, document: null });
  await terminal.sendInput('x');
  assert.equal(calls, 0, 'nothing is sent when no shell is attached');
});

test('output arriving from the callback is rendered as it happens', () => {
  const ui = build();
  createTerminal(ui.panel, { fetch: async () => ({ ok: true, json: async () => ({}) }), EventSource: null, document: null });
  ui.screen.children.length = 0;
  // A snapshot with a running shell, then one chunk of output.
  const terminal = createTerminal(ui.panel, { fetch: async () => ({ ok: true, json: async () => ({}) }), EventSource: null, document: null });
  terminal.handle({
    type: 'snapshot',
    shells: [{ id: 'shell-9', program: '/bin/sh', args: [], pty: true, state: 'running', opened_at: '', exit_code: null, scrollback: Buffer.from('welcome\n').toString('base64') }],
  });
  const html = ui.screen.children.map(child => child.html || '').join('');
  assert.match(html, /welcome/);
  terminal.handle({
    type: 'output',
    shell_id: 'shell-9',
    data: Buffer.from('uid=0(root)\n').toString('base64'),
    stderr: false,
  });
  assert.match(ui.screen.children.map(child => child.html || '').join(''), /uid=0\(root\)/);
});

test('a closed shell is reported with its exit status and stops accepting input', async () => {
  const ui = build();
  let sent = 0;
  const request = async () => { sent += 1; return { ok: true, json: async () => ({}) }; };
  const terminal = createTerminal(ui.panel, { fetch: request, EventSource: null, document: null });
  terminal.handle({
    type: 'snapshot',
    shells: [{ id: 'shell-2', program: '/bin/sh', args: [], pty: false, state: 'running', opened_at: '', exit_code: null, scrollback: '' }],
  });
  terminal.handle({ type: 'closed', shell_id: 'shell-2', code: 0 });
  assert.match(ui.message.textContent, /exited with status 0/);
  assert.equal(ui.input.disabled, true, 'a dead shell takes no more input');
  await terminal.sendInput('ls');
  assert.equal(sent, 0);
});