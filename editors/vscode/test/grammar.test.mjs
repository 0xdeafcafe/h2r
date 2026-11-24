// Tokenizes a sample script with the TextMate grammar, as VS Code does
// (vscode-textmate over vscode-oniguruma), and checks the scopes.

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
const grammarPath = join(root, 'syntaxes', 'rules.tmLanguage.json');

const wasm = readFileSync(join(dirname(require.resolve('vscode-oniguruma')), 'onig.wasm'));
await onig.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));

const registry = new vsctm.Registry({
  onigLib: Promise.resolve({
    createOnigScanner: (patterns) => new onig.OnigScanner(patterns),
    createOnigString: (s) => new onig.OnigString(s),
  }),
  loadGrammar: async (scopeName) =>
    scopeName === 'source.rules' ? vsctm.parseRawGrammar(readFileSync(grammarPath, 'utf8'), grammarPath) : null,
});
const grammar = await registry.loadGrammar('source.rules');

const source = [
  /* 0 */ 'rules "Demo" 1',
  /* 1 */ 'type slayer',
  /* 2 */ 'requires hud.message, player.weapons',
  /* 3 */ 'label goal: zones at most 4 per team = "goal"',
  /* 4 */ 'global var clock: timer',
  /* 5 */ 'option wait: time 5s..60s = 20s "Wait" "Seconds to wait"',
  /* 6 */ 'on kill(victim, killer) {',
  /* 7 */ '    if let k = killer and k != victim {',
  /* 8 */ '        message(victim, "{k} scored\\n")',
  /* 9 */ '        p.score = 1.5 + 300t - 1.5s * 42 // a comment',
  /* 10 */ '    }',
  /* 11 */ '    if victim is goal { }',
  /* 12 */ '    let x = 5st',
  /* 13 */ '    if variant.team_play == true and r == Role.vip { }',
  /* 14 */ '}',
  /* 15 */ 'uses slayer',
  /* 16 */ 'about "No shields."',
  /* 17 */ 'quick score_limit, shields',
  /* 18 */ 'settings {',
  /* 19 */ '    score_limit = 50',
  /* 20 */ '    shields = none; time_limit = 300s',
  /* 21 */ '    starting_weapon = battle_rifle // the rifle',
  /* 22 */ '}',
  /* 23 */ 'on tick {',
  /* 24 */ '    let about = 1',
  /* 25 */ '    quick = about',
  /* 26 */ '}',
  /* 27 */ 'points "Laps"',
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

function hasScope(n, needle, scope, nth = 0) {
  const tok = tokenAt(n, needle, nth);
  assert.ok(
    tok.scopes.includes(scope),
    `line ${n} "${needle}": token "${tok.text}" has [${tok.scopes.join(', ')}], not ${scope}`,
  );
  return tok;
}

function lacksScope(n, needle, scope, nth = 0) {
  const tok = tokenAt(n, needle, nth);
  assert.ok(
    !tok.scopes.includes(scope),
    `line ${n} "${needle}": token "${tok.text}" should not have ${scope}`,
  );
  return tok;
}

/** The token is exactly `needle` and has `scope`. */
function exact(n, needle, scope, nth = 0) {
  const tok = hasScope(n, needle, scope, nth);
  assert.equal(tok.text, needle, `line ${n}: token "${tok.text}" is not "${needle}"`);
}

test('every scope name ends in .rules', () => {
  for (const { tokens } of lines) {
    for (const t of tokens) {
      for (const s of t.scopes.slice(1)) {
        for (const part of s.split(' ')) {
          assert.ok(part.endsWith('.rules'), `scope ${part}`);
        }
      }
    }
  }
});

test('keywords', () => {
  exact(0, 'rules', 'keyword.other.declaration.rules');
  exact(1, 'type', 'keyword.other.declaration.rules');
  exact(6, 'on', 'keyword.other.declaration.rules');
  exact(7, 'if', 'keyword.control.conditional.rules');
  exact(7, 'let', 'keyword.other.declaration.rules');
  exact(7, 'and', 'keyword.operator.word.logical.rules');
  exact(11, 'is', 'keyword.operator.word.is.rules');
  exact(4, 'global', 'storage.modifier.scope.rules');
});

test('the header name, the type and capabilities', () => {
  exact(0, 'Demo', 'entity.name.section.rules');
  exact(1, 'slayer', 'support.constant.type.rules');
  exact(2, 'hud.message', 'support.constant.capability.rules');
  exact(2, 'player.weapons', 'support.constant.capability.rules');
});

test('types', () => {
  exact(4, 'timer', 'storage.type.rules');
  exact(5, 'time', 'storage.type.rules');
  exact(3, 'zones', 'storage.type.set.rules');
  exact(3, 'team', 'storage.type.rules');
  exact(3, 'at', 'keyword.other.requirement.rules');
  exact(3, 'most', 'keyword.other.requirement.rules');
  exact(3, 'per', 'keyword.other.requirement.rules');
});

test('a string with a hole and an escape', () => {
  hasScope(8, '"{k}', 'string.quoted.double.rules');
  hasScope(8, '"{k}', 'punctuation.definition.string.begin.rules');
  exact(8, '{', 'punctuation.definition.template-expression.begin.rules');
  const hole = hasScope(8, 'k}', 'variable.other.readwrite.rules');
  assert.ok(hole.scopes.includes('meta.template.expression.rules'));
  assert.ok(hole.scopes.includes('string.quoted.double.rules'));
  exact(8, '}', 'punctuation.definition.template-expression.end.rules');
  exact(8, '\\n', 'constant.character.escape.rules');
  const plain = hasScope(8, ' scored', 'string.quoted.double.rules');
  assert.ok(!plain.scopes.includes('meta.template.expression.rules'));
  lacksScope(8, ')', 'string.quoted.double.rules');
});

test('numbers and time literals', () => {
  exact(0, '1', 'constant.numeric.integer.rules');
  exact(9, '42', 'constant.numeric.integer.rules');
  exact(9, '1.5', 'constant.numeric.fixed.rules');
  exact(9, '300t', 'constant.numeric.time.rules');
  exact(9, '1.5s', 'constant.numeric.time.rules');
  exact(5, '5s', 'constant.numeric.time.rules');
  exact(5, '60s', 'constant.numeric.time.rules');
  exact(5, '20s', 'constant.numeric.time.rules');
  exact(5, '..', 'keyword.operator.range.rules');
});

test('5st is not a time literal', () => {
  const five = lacksScope(12, '5st', 'constant.numeric.time.rules');
  assert.equal(five.text, '5');
  assert.ok(five.scopes.includes('constant.numeric.integer.rules'));
  const st = tokenAt(12, 'st');
  assert.ok(!st.scopes.some((s) => s.startsWith('constant.numeric')), `st has [${st.scopes.join(', ')}]`);
});

test('a comment', () => {
  hasScope(9, '// a comment', 'comment.line.double-slash.rules');
  hasScope(9, 'a comment', 'comment.line.double-slash.rules');
});

test('operators', () => {
  exact(9, '=', 'keyword.operator.assignment.rules');
  exact(9, '+', 'keyword.operator.arithmetic.rules');
  exact(9, '-', 'keyword.operator.arithmetic.rules');
  exact(9, '*', 'keyword.operator.arithmetic.rules');
  exact(7, '!=', 'keyword.operator.comparison.rules');
  exact(13, '==', 'keyword.operator.comparison.rules');
});

test('labels, handlers and calls', () => {
  exact(3, 'goal', 'entity.name.label.rules');
  exact(11, 'goal', 'entity.name.label.rules');
  exact(6, 'kill', 'entity.name.function.event.rules');
  exact(8, 'message', 'support.function.rules');
  exact(4, 'clock', 'variable.other.definition.rules');
  exact(5, 'wait', 'variable.other.constant.option.rules');
  exact(7, 'k', 'variable.other.local.rules');
  exact(9, 'score', 'variable.other.property.rules');
});

test('enum and variant access', () => {
  exact(13, 'variant', 'variable.language.variant.rules');
  exact(13, 'team_play', 'variable.other.property.variant.rules');
  exact(13, 'Role', 'entity.name.type.enum.rules');
  exact(13, 'vip', 'variable.other.enummember.rules');
});

test('the header lines uses, points, about and quick', () => {
  exact(15, 'uses', 'keyword.other.declaration.rules');
  exact(15, 'slayer', 'support.constant.type.rules');
  exact(27, 'points', 'keyword.other.declaration.rules');
  hasScope(27, 'Laps', 'string.quoted.double.rules');
  exact(16, 'about', 'keyword.other.declaration.rules');
  hasScope(16, 'No shields.', 'string.quoted.double.rules');
  exact(17, 'quick', 'keyword.other.declaration.rules');
  exact(17, 'score_limit', 'variable.other.property.setting.rules');
  exact(17, 'shields', 'variable.other.property.setting.rules');
  hasScope(17, 'shields', 'meta.quick.rules');
});

test('the settings block', () => {
  exact(18, 'settings', 'keyword.other.declaration.rules');
  exact(18, '{', 'punctuation.section.braces.begin.rules');
  exact(19, 'score_limit', 'variable.other.property.setting.rules');
  hasScope(19, 'score_limit', 'meta.settings.rules');
  exact(19, '50', 'constant.numeric.integer.rules');
  exact(20, 'shields', 'variable.other.property.setting.rules');
  exact(20, 'none', 'constant.language.none.rules');
  exact(20, 'time_limit', 'variable.other.property.setting.rules');
  exact(20, '300s', 'constant.numeric.time.rules');
  exact(21, 'starting_weapon', 'variable.other.property.setting.rules');
  exact(21, 'battle_rifle', 'variable.other.enummember.rules');
  hasScope(21, 'the rifle', 'comment.line.double-slash.rules');
  exact(22, '}', 'punctuation.section.braces.end.rules');
  lacksScope(23, 'on', 'meta.settings.rules');
});

test('the four header words are names in a handler', () => {
  lacksScope(24, 'about', 'keyword.other.declaration.rules');
  lacksScope(25, 'quick', 'keyword.other.declaration.rules');
  lacksScope(25, 'about', 'keyword.other.declaration.rules');
});
