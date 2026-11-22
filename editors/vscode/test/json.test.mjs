// Every JSON file the extension ships parses, its regexes compile, and the
// manifest agrees with the language server.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (rel) => JSON.parse(readFileSync(join(root, rel), 'utf8'));

// crates/formats/rules/src/ide/mod.rs, LEGEND.
const LEGEND = [
  'keyword',
  'event',
  'function',
  'api',
  'variable',
  'parameter',
  'enum',
  'enumMember',
  'type',
  'number',
  'string',
  'comment',
  'operator',
  'label',
  'capability',
];

const jsonFiles = [
  'package.json',
  'language-configuration.json',
  'tsconfig.json',
  ...readdirSync(join(root, 'syntaxes'))
    .filter((f) => f.endsWith('.json'))
    .map((f) => `syntaxes/${f}`),
  ...readdirSync(join(root, 'snippets'))
    .filter((f) => f.endsWith('.json'))
    .map((f) => `snippets/${f}`),
];

test('every JSON file parses', () => {
  assert.ok(jsonFiles.includes('syntaxes/rules.tmLanguage.json'));
  assert.ok(jsonFiles.includes('snippets/rules.json'));
  for (const f of jsonFiles) {
    assert.doesNotThrow(() => read(f), f);
  }
});

test('the language configuration regexes compile', () => {
  const conf = read('language-configuration.json');
  const patterns = [
    conf.wordPattern,
    conf.indentationRules.increaseIndentPattern,
    conf.indentationRules.decreaseIndentPattern,
    conf.folding.markers.start,
    conf.folding.markers.end,
  ];
  for (const rule of conf.onEnterRules) {
    patterns.push(rule.beforeText);
    if (rule.afterText) patterns.push(rule.afterText);
    if (rule.previousLineText) patterns.push(rule.previousLineText);
  }
  for (const p of patterns) {
    assert.equal(typeof p, 'string');
    assert.doesNotThrow(() => new RegExp(p), p);
    assert.doesNotThrow(() => new RegExp(p, 'u'), p);
  }
  assert.equal(conf.comments.lineComment, '//');
});

test('the language configuration behaves', () => {
  const conf = read('language-configuration.json');
  const inc = new RegExp(conf.indentationRules.increaseIndentPattern);
  const dec = new RegExp(conf.indentationRules.decreaseIndentPattern);
  assert.ok(inc.test('on kill(victim, killer) {'));
  assert.ok(inc.test('table t: (a: int) = ['));
  assert.ok(inc.test('if x { // why'));
  assert.ok(!inc.test('// a comment {'));
  assert.ok(!inc.test('if x { y }'));
  assert.ok(dec.test('    }'));
  assert.ok(dec.test(']'));
  assert.ok(!dec.test('x }'));
  const word = new RegExp(`^(?:${conf.wordPattern})$`);
  for (const w of ['killer', 'round_start', '5s', '300t', '1.5s', '42', '1.5']) {
    assert.ok(word.test(w), w);
  }
  const start = new RegExp(conf.folding.markers.start);
  const end = new RegExp(conf.folding.markers.end);
  assert.ok(start.test('// region scoring'));
  assert.ok(start.test('  //#region'));
  assert.ok(end.test('// endregion'));
  assert.ok(!start.test('// endregion'));
});

test('semanticTokenScopes cover the server legend', () => {
  const pkg = read('package.json');
  const entry = pkg.contributes.semanticTokenScopes.find((s) => s.language === 'rules');
  assert.ok(entry, 'a semanticTokenScopes entry for rules');
  for (const name of LEGEND) {
    const scopes = entry.scopes[name];
    assert.ok(Array.isArray(scopes) && scopes.length > 0, `semantic token ${name} has scopes`);
    for (const s of scopes) {
      assert.ok(s.endsWith('.rules'), `${name}: ${s}`);
    }
  }
  const custom = pkg.contributes.semanticTokenTypes.map((t) => t.id).sort();
  assert.deepEqual(custom, ['api', 'capability', 'label']);
});

test('the manifest is consistent', () => {
  const pkg = read('package.json');
  assert.equal(pkg.name, 'h2-rules');
  assert.equal(pkg.publisher, 'h2recomp');
  assert.equal(pkg.license, 'WTFPL');
  assert.equal(pkg.main, './out/extension.js');
  assert.deepEqual(pkg.activationEvents, ['onLanguage:rules']);
  const lang = pkg.contributes.languages.find((l) => l.id === 'rules');
  assert.deepEqual(lang.extensions, ['.rules']);
  const grammar = pkg.contributes.grammars.find((g) => g.language === 'rules');
  assert.equal(grammar.scopeName, 'source.rules');
  assert.equal(read(grammar.path.replace(/^\.\//, '')).scopeName, 'source.rules');
  read(lang.configuration.replace(/^\.\//, ''));
  for (const s of pkg.contributes.snippets) {
    read(s.path.replace(/^\.\//, ''));
  }
  const commands = pkg.contributes.commands.map((c) => c.command).sort();
  assert.deepEqual(commands, ['h2rules.restartServer', 'h2rules.showOutput']);
  const props = pkg.contributes.configuration.properties;
  assert.equal(props['h2rules.serverPath'].default, '');
  assert.deepEqual(props['h2rules.trace.server'].enum, ['off', 'messages', 'verbose']);
  // vsce refuses @types/vscode newer than engines.vscode.
  const engine = pkg.engines.vscode.replace(/^[\^~]/, '');
  const types = pkg.devDependencies['@types/vscode'].replace(/^[\^~]/, '');
  assert.equal(types, engine);
});

test('snippets have a prefix and a body', () => {
  const snippets = read('snippets/rules.json');
  for (const [name, s] of Object.entries(snippets)) {
    assert.ok(typeof s.prefix === 'string' && s.prefix.length > 0, name);
    assert.ok(Array.isArray(s.body) && s.body.length > 0, name);
  }
});

test('every grammar regex is a string', () => {
  const grammar = read('syntaxes/rules.tmLanguage.json');
  const walk = (node) => {
    if (Array.isArray(node)) {
      node.forEach(walk);
    } else if (node && typeof node === 'object') {
      for (const key of ['match', 'begin', 'end']) {
        if (key in node) assert.equal(typeof node[key], 'string', key);
      }
      for (const key of ['name', 'contentName']) {
        if (key in node) {
          for (const part of node[key].split(' ')) assert.ok(part.endsWith('.rules'), part);
        }
      }
      Object.values(node).forEach(walk);
    }
  };
  walk(grammar.repository);
  walk(grammar.patterns);
});
