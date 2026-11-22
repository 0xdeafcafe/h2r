// The server lookup (src/serverPath.ts), through its compiled output:
// `npm test` runs `tsc -p .` first.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { resolveServerPath, expandHome, pathCandidates, executableNames } = require('../out/serverPath.js');

const existing = (...files) => {
  const set = new Set(files);
  return (f) => set.has(f);
};

test('expandHome', () => {
  assert.equal(expandHome('~', '/home/a', 'linux'), '/home/a');
  assert.equal(expandHome('~/bin/h2', '/home/a', 'linux'), '/home/a/bin/h2');
  assert.equal(expandHome('/opt/h2', '/home/a', 'linux'), '/opt/h2');
  assert.equal(expandHome('~other/h2', '/home/a', 'linux'), '~other/h2');
  assert.equal(expandHome('~\\bin\\h2.exe', 'C:\\Users\\a', 'win32'), 'C:\\Users\\a\\bin\\h2.exe');
});

test('a setting wins and has ~ expanded', () => {
  const r = resolveServerPath('~/src/h2', '/usr/bin', '/home/a', existing('/home/a/src/h2', '/usr/bin/h2'), 'linux');
  assert.deepEqual(r, { found: true, path: '/home/a/src/h2', source: 'setting' });
});

test('a setting that names nothing fails and names it', () => {
  const r = resolveServerPath('/nope/h2', '/usr/bin', '/home/a', existing('/usr/bin/h2'), 'darwin');
  assert.equal(r.found, false);
  assert.equal(r.setting, '/nope/h2');
  assert.deepEqual(r.tried, ['/nope/h2']);
});

test('a bare name in the setting is looked up on PATH', () => {
  const r = resolveServerPath('h2-dev', '/a:/b', '/home/a', existing('/b/h2-dev'), 'linux');
  assert.deepEqual(r, { found: true, path: '/b/h2-dev', source: 'setting' });
});

test('a blank setting falls through to PATH, in order', () => {
  const r = resolveServerPath('  ', '/a::/b:/c', '/home/a', existing('/b/h2', '/c/h2'), 'darwin');
  assert.deepEqual(r, { found: true, path: '/b/h2', source: 'path' });
});

test('then ~/.cargo/bin/h2', () => {
  const r = resolveServerPath('', '/a:/b', '/home/a', existing('/home/a/.cargo/bin/h2'), 'linux');
  assert.deepEqual(r, { found: true, path: '/home/a/.cargo/bin/h2', source: 'cargo' });
  const none = resolveServerPath(undefined, undefined, '/home/a', existing(), 'linux');
  assert.equal(none.found, false);
  assert.equal(none.setting, undefined);
  assert.deepEqual(none.tried, ['/home/a/.cargo/bin/h2']);
});

test('nothing found lists every place tried', () => {
  const r = resolveServerPath('', '/a:/b', '/home/a', existing(), 'linux');
  assert.deepEqual(r, { found: false, tried: ['/a/h2', '/b/h2', '/home/a/.cargo/bin/h2'] });
});

test('Windows: PATHEXT, ; separators, quoted entries and h2.exe', () => {
  assert.deepEqual(executableNames('h2', 'win32', '.EXE;.CMD'), ['h2.exe', 'h2.cmd']);
  assert.deepEqual(executableNames('h2', 'linux', '.EXE'), ['h2']);
  assert.deepEqual(pathCandidates('h2', 'C:\\a;"C:\\b c"', 'win32', '.EXE'), ['C:\\a\\h2.exe', 'C:\\b c\\h2.exe']);
  const onPath = resolveServerPath('', 'C:\\a;C:\\b', 'C:\\Users\\a', existing('C:\\b\\h2.exe'), 'win32', '.COM;.EXE');
  assert.deepEqual(onPath, { found: true, path: 'C:\\b\\h2.exe', source: 'path' });
  const cargo = resolveServerPath('', '', 'C:\\Users\\a', existing('C:\\Users\\a\\.cargo\\bin\\h2.exe'), 'win32');
  assert.deepEqual(cargo, { found: true, path: 'C:\\Users\\a\\.cargo\\bin\\h2.exe', source: 'cargo' });
  const setting = resolveServerPath('C:\\tools\\h2', '', 'C:\\Users\\a', existing('C:\\tools\\h2.exe'), 'win32', '.EXE');
  assert.deepEqual(setting, { found: true, path: 'C:\\tools\\h2.exe', source: 'setting' });
});
