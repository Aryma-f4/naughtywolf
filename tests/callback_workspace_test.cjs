const { test } = require('node:test');
const assert = require('node:assert/strict');
const { createWorkspace } = require('../static/callback-workspace.js');

class Element {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.dataset = {};
    this.attributes = {};
    this.listeners = {};
    this.className = '';
    this.textContent = '';
    this.value = '';
    this.hidden = false;
    this.disabled = false;
    this.parentNode = null;
  }
  append(...children) { for (const child of children) { child.parentNode = this; this.children.push(child); } }
  prepend(child) { child.parentNode = this; this.children.unshift(child); }
  replaceChildren(...children) { this.children = []; this.append(...children); }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name] ?? null; }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
  dispatch(type, values = {}) {
    const event = { target: this, preventDefault() { this.defaultPrevented = true; }, ...values };
    for (const listener of this.listeners[type] || []) listener(event);
    return event;
  }
  matches(selector) {
    if (selector.startsWith('[data-')) {
      const key = selector.slice(6, -1).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
      return key in this.dataset;
    }
    if (selector.startsWith('.')) return this.className.split(/\s+/).includes(selector.slice(1));
    if (selector.startsWith('#')) return this.attributes.id === selector.slice(1);
    return this.tagName.toLowerCase() === selector.toLowerCase();
  }
  querySelectorAll(selector) {
    const found = [];
    const visit = node => {
      for (const child of node.children) {
        if (child.matches(selector)) found.push(child);
        visit(child);
      }
    };
    visit(this);
    return found;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  closest(selector) {
    let node = this;
    while (node) { if (node.matches(selector)) return node; node = node.parentNode; }
    return null;
  }
}

const document = { createElement: tag => new Element(tag) };

function skeleton() {
  const root = new Element('section');
  root.dataset.callbackTasking = '';
  root.dataset.tasksEndpoint = '/api/callbacks/callback-1/tasks';
  root.dataset.eventsEndpoint = '/api/callbacks/callback-1/events';
  root.dataset.csrfToken = 'csrf';
  root.dataset.online = 'false';
  for (const key of ['taskList', 'taskEmpty', 'taskSearch', 'taskStateFilter', 'taskErrorFilter', 'loadOlder', 'connectionState', 'offlineQueue', 'commandInput', 'commandArguments', 'taskForm']) {
    const element = new Element(key === 'taskForm' ? 'form' : key.includes('Filter') ? 'select' : key.includes('Input') || key === 'taskSearch' || key === 'commandArguments' ? 'input' : 'div');
    element.dataset[key] = '';
    root.append(element);
  }
  return root;
}

function task(overrides = {}) {
  return {
    id: 'task-1', session_id: 'callback-1', command: 'whoami', arguments: [], timeout_ms: 30000,
    status: 'pending', state_label: 'Queued', state_class: 'neutral', operator_id: 'op-1',
    operator_name: 'alice', parent_task_id: null, created_at: '2026-09-10T12:00:00Z',
    updated_at: '2026-09-10T12:00:00Z', processing_at: null, completed_at: null,
    cancellation_requested_at: null, stdout: null, stderr: null, exit_code: null, ...overrides,
  };
}

function dependencies(pages = [{ tasks: [], next_before: null }]) {
  let index = 0;
  const deps = {};
  class EventSource {
    constructor() { this.listeners = {}; deps.source = this; }
    addEventListener(type, listener) { this.listeners[type] = listener; }
    close() {}
  }
  Object.assign(deps, {
    document, EventSource,
    fetch: async () => ({ ok: true, json: async () => pages[Math.min(index++, pages.length - 1)] }),
  });
  return deps;
}

const flush = () => new Promise(resolve => setImmediate(resolve));

test('same task snapshots dedupe by ID and update the existing card state', async () => {
  const root = skeleton();
  const workspace = createWorkspace(root, dependencies());
  await workspace.ready;
  workspace.upsert(task());
  const firstCard = root.querySelector('[data-task-card]');
  workspace.upsert(task({ status: 'completed', state_label: 'Completed', state_class: 'success', stdout: 'root', exit_code: 0 }));
  assert.equal(root.querySelectorAll('[data-task-card]').length, 1);
  assert.equal(root.querySelector('[data-task-card]'), firstCard);
  assert.equal(firstCard.dataset.taskStatus, 'completed');
  assert.equal(firstCard.querySelector('[data-task-state]').textContent, 'Completed');
  assert.equal(firstCard.querySelector('[data-task-stdout]').textContent, 'root');
});

test('authoritative reload data restores prior output', async () => {
  const root = skeleton();
  const workspace = createWorkspace(root, dependencies([{ tasks: [task({ status: 'completed', state_label: 'Completed', stdout: 'persisted output', exit_code: 0 })], next_before: null }]));
  await workspace.ready;
  assert.equal(root.querySelectorAll('[data-task-card]').length, 1);
  assert.equal(root.querySelector('[data-task-stdout]').textContent, 'persisted output');
});

test('filters hide cards without deleting task history', async () => {
  const root = skeleton();
  const workspace = createWorkspace(root, dependencies());
  await workspace.ready;
  workspace.upsert(task({ id: 'ok', command: 'hostname', status: 'completed', state_label: 'Completed', stdout: 'lab' }));
  workspace.upsert(task({ id: 'bad', command: 'cat', status: 'error', state_label: 'Error', stderr: 'denied' }));
  const search = root.querySelector('[data-task-search]');
  search.value = 'hostname';
  search.dispatch('input');
  assert.equal(root.querySelectorAll('[data-task-card]').length, 2);
  assert.equal(root.querySelectorAll('[data-task-card]').filter(card => !card.hidden).length, 1);
  search.value = '';
  search.dispatch('input');
  assert.equal(root.querySelectorAll('[data-task-card]').filter(card => !card.hidden).length, 2);
});

test('ArrowUp selects the newest persisted command after reload', async () => {
  const root = skeleton();
  const newest = task({ id: 'new', command: 'uname', created_at: '2026-09-10T12:01:00Z' });
  const older = task({ id: 'old', command: 'pwd', created_at: '2026-09-10T12:00:00Z' });
  const workspace = createWorkspace(root, dependencies([{ tasks: [newest, older], next_before: null }]));
  await workspace.ready;
  const input = root.querySelector('[data-command-input]');
  const event = input.dispatch('keydown', { key: 'ArrowUp' });
  assert.equal(event.defaultPrevented, true);
  assert.equal(input.value, 'uname');
});

test('SSE reconnect refetches the first page and upserts by ID', async () => {
  const root = skeleton();
  const deps = dependencies([
    { tasks: [task()], next_before: null },
    { tasks: [task({ status: 'completed', state_label: 'Completed', stdout: 'after reconnect' })], next_before: null },
  ]);
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  deps.source.onerror();
  deps.source.onopen();
  await flush();
  assert.equal(root.querySelectorAll('[data-task-card]').length, 1);
  assert.equal(root.querySelector('[data-task-stdout]').textContent, 'after reconnect');
});
