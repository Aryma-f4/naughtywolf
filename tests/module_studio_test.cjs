const test = require('node:test');
const assert = require('node:assert/strict');

const { encodeUtf8Base64, buildModuleTask } = require('../static/module_studio.js');

test('encodes unicode custom source as UTF-8 base64', () => {
  const encoded = encodeUtf8Base64('echo halo 🐺');
  assert.equal(Buffer.from(encoded, 'base64').toString('utf8'), 'echo halo 🐺');
});

test('builds fixed quick-action task payloads', () => {
  assert.deepEqual(buildModuleTask('nw/user-enum', 45000), {
    command: 'nw/user-enum', args: [], timeout_ms: 45000,
  });
  assert.throws(() => buildModuleTask('whoami', 30000), /module command/);
});

test('builds custom code payload with bounded source', () => {
  const task = buildModuleTask('nw/exec-code', 60000, 'python', 'print("wolf")');
  assert.equal(task.command, 'nw/exec-code');
  assert.equal(task.args[0], 'python');
  assert.equal(Buffer.from(task.args[1], 'base64').toString('utf8'), 'print("wolf")');
  assert.throws(
    () => buildModuleTask('nw/exec-code', 60000, 'shell', 'a'.repeat(24577)),
    /24 KiB/
  );
});
