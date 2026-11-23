// Tokenizes a sample HaloScript working copy with the TextMate grammar, as
// VS Code does (vscode-textmate over vscode-oniguruma), and checks the
// scopes. The names in the sample are invented.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const vsctm = require('vscode-textmate');
const onig = require('vscode-oniguruma');

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const grammarPath = join(root, 'syntaxes', 'haloscript.tmLanguage.json');

const wasm = readFileSync(join(dirname(require.resolve('vscode-oniguruma')), 'onig.wasm'));
await onig.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));

const registry = new vsctm.Registry({
  onigLib: Promise.resolve({
    createOnigScanner: (patterns) => new onig.OnigScanner(patterns),
    createOnigString: (s) => new onig.OnigString(s),
  }),
  loadGrammar: async (scopeName) =>
    scopeName === 'source.haloscript'
      ? vsctm.parseRawGrammar(readFileSync(grammarPath, 'utf8'), grammarPath)
      : null,
});
const grammar = await registry.loadGrammar('source.haloscript');

const source = [
  /* 0 */ '; a line comment (with parens)',
  /* 1 */ ';* a block',
  /* 2 */ '   comment *;',
  /* 3 */ '(global short count 3)',
  /* 4 */ '(script startup main',
  /* 5 */ '  (begin (invented_print "hi \\"there\\"") (invented_wait -30)))',
  /* 6 */ '(script static void helper (set count 1.5))',
  /* 7 */ '(if true (door_open none))',
  /* 8 */ '(global real ratio false)',
];

const lines = [];
{
  let stack = vsctm.INITIAL;
  for (const text of source) {
    const r = grammar.tokenizeLine(text, stack);
    lines.push({ text, tokens: r.tokens });
    stack = r.ruleStack;
  }
}

/** The token at the `nth` occurrence of `needle` on line `n`. */
function tokenAt(n, needle, nth = 0) {
  const { text, tokens } = lines[n];
  let at = -1;
  for (let i = 0; i <= nth; i++) {
    at = text.indexOf(needle, at + 1);
    assert.notEqual(at, -1, `"${needle}" (#${nth}) is not on line ${n}: ${text}`);
  }
  const tok = tokens.find((t) => t.startIndex <= at && at < t.endIndex);
  assert.ok(tok, `no token at ${at} on line ${n}`);
  return { text: text.slice(tok.startIndex, tok.endIndex), scopes: tok.scopes };
}

/** The token is exactly `needle` and has `scope`. */
function exact(n, needle, scope, nth = 0) {
  const tok = tokenAt(n, needle, nth);
  assert.ok(
    tok.scopes.includes(scope),
    `line ${n} "${needle}": token "${tok.text}" has [${tok.scopes.join(', ')}], not ${scope}`,
  );
  assert.equal(tok.text, needle, `line ${n}: token "${tok.text}" is not "${needle}"`);
}

function hasScope(n, needle, scope, nth = 0) {
  const tok = tokenAt(n, needle, nth);
  assert.ok(
    tok.scopes.includes(scope),
    `line ${n} "${needle}": token "${tok.text}" has [${tok.scopes.join(', ')}], not ${scope}`,
  );
}

test('every scope name ends in .haloscript', () => {
  for (const { tokens } of lines) {
    for (const t of tokens) {
      for (const s of t.scopes.slice(1)) {
        assert.ok(s.endsWith('.haloscript'), `scope ${s}`);
      }
    }
  }
});

test('comments, line and block', () => {
  hasScope(0, 'parens', 'comment.line.semicolon.haloscript');
  hasScope(1, 'block', 'comment.block.haloscript');
  hasScope(2, 'comment', 'comment.block.haloscript');
  // The block ends at `*;`; the next line is code again.
  exact(3, 'global', 'keyword.other.haloscript');
});

test('global headers', () => {
  exact(3, 'global', 'keyword.other.haloscript');
  exact(3, 'short', 'storage.type.haloscript');
  exact(3, 'count', 'variable.other.global.haloscript');
  exact(3, '3', 'constant.numeric.haloscript');
  exact(8, 'real', 'storage.type.haloscript');
  exact(8, 'false', 'constant.language.haloscript');
});

test('script headers', () => {
  exact(4, 'script', 'keyword.other.haloscript');
  exact(4, 'startup', 'keyword.other.script-kind.haloscript');
  exact(4, 'main', 'entity.name.function.script.haloscript');
  exact(6, 'static', 'keyword.other.script-kind.haloscript');
  exact(6, 'void', 'storage.type.haloscript');
  exact(6, 'helper', 'entity.name.function.script.haloscript');
});

test('call heads, strings, numbers and constants', () => {
  exact(5, 'begin', 'entity.name.function.call.haloscript');
  exact(5, 'invented_print', 'entity.name.function.call.haloscript');
  exact(5, 'invented_wait', 'entity.name.function.call.haloscript');
  hasScope(5, 'hi', 'string.quoted.double.haloscript');
  exact(5, '\\"', 'constant.character.escape.haloscript');
  hasScope(5, 'there', 'string.quoted.double.haloscript');
  exact(5, '-30', 'constant.numeric.haloscript');
  exact(6, 'set', 'entity.name.function.call.haloscript');
  exact(6, '1.5', 'constant.numeric.haloscript');
  exact(7, 'if', 'entity.name.function.call.haloscript');
  exact(7, 'true', 'constant.language.haloscript');
  exact(7, 'door_open', 'entity.name.function.call.haloscript');
  exact(7, 'none', 'constant.language.haloscript');
});

test('parens are punctuation', () => {
  hasScope(5, '(', 'punctuation.section.parens.begin.haloscript');
  const tok = tokenAt(7, ')', 0);
  assert.ok(tok.scopes.includes('punctuation.section.parens.haloscript'));
});
