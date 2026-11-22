// The workbench editor's Monaco half (docs/game/rules-engine.md,
// "Workbench"), loaded only by RulesEditor's dynamic import so Monaco is a
// chunk of its own: the core editor (no bundled languages, one editor
// worker), the `h2rules` language with its Monarch grammar, the providers
// that ask `POST /v1/rules/ide`, the stats site's theme, and one editor
// mounted at a time. Everything it computes is in ../lib/rulesIde.ts.

import * as monaco from "monaco-editor/esm/vs/editor/edcore.main";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import type { Problem } from "../lib/rules";
import {
  DEFAULT_LEGEND, IdeClient, LANGUAGE_CONFIG, LANGUAGE_ID, MONARCH, completionKindName, encodeTokens, fetchLegend,
  fromLspRange, parseCompletions, parseFormat, parseHover, parseLocations, parseSignature, parseSymbols, parseTokens,
  problemMarker, symbolKindName, toLsp, type IdeOp, type IdeSymbol,
} from "../lib/rulesIde";

const THEME = "h2-stats";
const OWNER = "h2rules-check";

let apiBase = "";
/** The Live API the providers ask; the page sets it as its server changes. */
export function setApi(api: string): void { apiBase = api; }

const ide = new IdeClient(() => apiBase);

/** Monaco's cancellation token as an AbortSignal. */
function signalOf(token: monaco.CancellationToken): AbortSignal {
  const c = new AbortController();
  if (token.isCancellationRequested) c.abort();
  else token.onCancellationRequested(() => c.abort());
  return c.signal;
}

const ask = (op: IdeOp, model: monaco.editor.ITextModel, position: monaco.Position | null, token: monaco.CancellationToken) =>
  ide.request(op, model.getValue(), model.getVersionId(), position ? toLsp(position.lineNumber, position.column) : { line: 0, character: 0 }, signalOf(token));

/** A site colour (styles/theme.css) without its `#`, or the fallback. */
function siteColour(name: string, fallback: string): string {
  try {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    if (/^#[0-9a-f]{3,8}$/i.test(v)) return (v.length === 4 ? v.slice(1).split("").map((c) => c + c).join("") : v.slice(1)).toLowerCase();
  } catch { /* no document */ }
  return fallback;
}

function defineTheme(): void {
  const link = siteColour("--s-link", "71caef");
  const gold = siteColour("--s-gold", "f6b620");
  const good = siteColour("--s-good", "8fdc5d");
  const bad = siteColour("--s-bad", "f26122");
  const warn = siteColour("--s-warn", "ffc857");
  const ink = siteColour("--s-ink", "bbbbbb");
  const dim = siteColour("--s-ink-dim", "6f6f6f");
  const edge = siteColour("--s-head-edge", "4d76ad");
  const pill = siteColour("--s-pill-edge", "5f83b6");
  const hover = siteColour("--s-row-hover", "33404f");
  const head = siteColour("--s-head-2", "0e1f3d");
  const box = siteColour("--s-box", "2b2b2b");
  const sel = siteColour("--s-row-sel", "585858");
  monaco.editor.defineTheme(THEME, {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "", foreground: "e8e8e8" },
      { token: "comment", foreground: dim, fontStyle: "italic" },
      { token: "keyword", foreground: link, fontStyle: "bold" },
      { token: "keyword.control", foreground: link, fontStyle: "bold" },
      { token: "keyword.operator", foreground: link },
      { token: "type", foreground: pill },
      { token: "type.event", foreground: gold, fontStyle: "bold" },
      { token: "entity.name.function", foreground: gold },
      { token: "predefined", foreground: warn },
      { token: "variable.predefined", foreground: "9fb6d6" },
      { token: "constant", foreground: good },
      { token: "number", foreground: good },
      { token: "string", foreground: "e2b36d" },
      { token: "string.escape", foreground: warn },
      { token: "string.escape.invalid", foreground: bad },
      { token: "operator", foreground: ink },
      { token: "delimiter", foreground: ink },
      { token: "invalid", foreground: bad },
      // Semantic token types (the route's legend).
      { token: "function", foreground: warn },
      { token: "event", foreground: gold, fontStyle: "bold" },
      { token: "parameter", foreground: "c5d6ee" },
      { token: "variable", foreground: "e8e8e8" },
      { token: "property", foreground: "9fb6d6" },
      { token: "enumMember", foreground: good },
      { token: "namespace", foreground: pill },
    ],
    colors: {
      "editor.background": "#0b0b0b",
      "editor.foreground": "#e8e8e8",
      "editorLineNumber.foreground": "#5d6b80",
      "editorLineNumber.activeForeground": `#${ink}`,
      "editorGutter.background": "#121822",
      "editor.lineHighlightBackground": "#121822",
      "editor.selectionBackground": `#${sel}`,
      "editor.inactiveSelectionBackground": `#${hover}`,
      "editorCursor.foreground": `#${link}`,
      "editorIndentGuide.background1": "#1e2633",
      "editorBracketMatch.border": `#${edge}`,
      "editorError.foreground": `#${bad}`,
      "editorWarning.foreground": `#${warn}`,
      "editorWidget.background": `#${head}`,
      "editorWidget.border": `#${edge}`,
      "editorSuggestWidget.background": `#${head}`,
      "editorSuggestWidget.border": `#${edge}`,
      "editorSuggestWidget.selectedBackground": `#${hover}`,
      "editorHoverWidget.background": `#${head}`,
      "editorHoverWidget.border": `#${edge}`,
      "peekView.border": `#${edge}`,
      "peekViewEditor.background": "#0b0b0b",
      "peekViewResult.background": `#${box}`,
      "peekViewTitle.background": `#${head}`,
      "input.background": "#111111",
      "focusBorder": `#${pill}`,
      "scrollbarSlider.background": "#40404080",
    },
  });
}

function symbolOf(s: IdeSymbol): monaco.languages.DocumentSymbol {
  return {
    name: s.name,
    detail: "",
    kind: monaco.languages.SymbolKind[symbolKindName(s.kind)],
    tags: [],
    range: fromLspRange(s.range),
    selectionRange: fromLspRange(s.selectionRange),
    children: s.children.map(symbolOf),
  };
}

function registerLanguage(legend: string[]): void {
  monaco.languages.register({ id: LANGUAGE_ID, extensions: [".rules"], aliases: ["Halo 2 rules", LANGUAGE_ID] });
  monaco.languages.setMonarchTokensProvider(LANGUAGE_ID, MONARCH as unknown as monaco.languages.IMonarchLanguage);
  monaco.languages.setLanguageConfiguration(LANGUAGE_ID, LANGUAGE_CONFIG as unknown as monaco.languages.LanguageConfiguration);

  monaco.languages.registerCompletionItemProvider(LANGUAGE_ID, {
    triggerCharacters: ["."],
    provideCompletionItems: async (model, position, _context, token) => {
      const items = parseCompletions(await ask("complete", model, position, token));
      const word = model.getWordUntilPosition(position);
      const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
      return {
        suggestions: items.map((c): monaco.languages.CompletionItem => ({
          label: c.label,
          kind: monaco.languages.CompletionItemKind[completionKindName(c.kind)],
          detail: c.detail || undefined,
          documentation: c.documentation ? { value: c.documentation } : undefined,
          insertText: c.insertText,
          insertTextRules: c.snippet ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined,
          range,
        })),
      };
    },
  });

  monaco.languages.registerHoverProvider(LANGUAGE_ID, {
    provideHover: async (model, position, token) => {
      const h = parseHover(await ask("hover", model, position, token));
      return h ? { contents: [{ value: h.contents }], range: h.range ? fromLspRange(h.range) : undefined } : null;
    },
  });

  monaco.languages.registerSignatureHelpProvider(LANGUAGE_ID, {
    signatureHelpTriggerCharacters: ["(", ","],
    signatureHelpRetriggerCharacters: [","],
    provideSignatureHelp: async (model, position, token) => {
      const s = parseSignature(await ask("signature", model, position, token));
      if (!s) return null;
      return {
        value: {
          signatures: s.signatures.map((g) => ({
            label: g.label,
            documentation: g.documentation ? { value: g.documentation } : undefined,
            parameters: g.parameters.map((label) => ({ label })),
          })),
          activeSignature: s.activeSignature,
          activeParameter: s.activeParameter,
        },
        dispose: () => {},
      };
    },
  });

  monaco.languages.registerDefinitionProvider(LANGUAGE_ID, {
    provideDefinition: async (model, position, token) =>
      parseLocations(await ask("definition", model, position, token)).map((r) => ({ uri: model.uri, range: fromLspRange(r) })),
  });

  monaco.languages.registerReferenceProvider(LANGUAGE_ID, {
    provideReferences: async (model, position, _context, token) =>
      parseLocations(await ask("references", model, position, token)).map((r) => ({ uri: model.uri, range: fromLspRange(r) })),
  });

  monaco.languages.registerDocumentSymbolProvider(LANGUAGE_ID, {
    displayName: "Game type script",
    provideDocumentSymbols: async (model, token) => parseSymbols(await ask("symbols", model, null, token)).map(symbolOf),
  });

  monaco.languages.registerDocumentFormattingEditProvider(LANGUAGE_ID, {
    displayName: "Game type script",
    provideDocumentFormattingEdits: async (model, _options, token) => {
      const f = parseFormat(await ask("format", model, null, token));
      if (!f || "error" in f || f.text === model.getValue()) return [];
      return [{ range: model.getFullModelRange(), text: f.text }];
    },
  });

  monaco.languages.registerDocumentSemanticTokensProvider(LANGUAGE_ID, {
    getLegend: () => ({ tokenTypes: legend, tokenModifiers: [] }),
    provideDocumentSemanticTokens: async (model, _lastResultId, token) => {
      const t = parseTokens(await ask("tokens", model, null, token));
      return t ? { data: new Uint32Array(encodeTokens(t.data, legend, t.legend)) } : null;
    },
    releaseDocumentSemanticTokens: () => {},
  });
}

let ready: Promise<void> | null = null;

/** Sets Monaco up once: its worker, the theme, the language. */
export function setup(): Promise<void> {
  ready ??= (async () => {
    (globalThis as unknown as { MonacoEnvironment?: { getWorker(): Worker } }).MonacoEnvironment = { getWorker: () => new EditorWorker() };
    defineTheme();
    const legend = (await fetchLegend(apiBase)) ?? [...DEFAULT_LEGEND];
    registerLanguage(legend);
  })();
  return ready;
}

/** One mounted editor, as RulesEditor drives it. */
export interface Mounted {
  setValue(v: string): void;
  setReadOnly(ro: boolean): void;
  setProblems(ps: readonly Problem[]): void;
  /** Selects a check error's place and scrolls it into view. */
  reveal(p: Problem): void;
  /** Inserts a snippet at the caret, its placeholders live. */
  insert(snippet: string): void;
  focus(): void;
  dispose(): void;
}

interface SnippetController { insert(template: string): void }

export function mount(el: HTMLElement, value: string, readOnly: boolean, onChange: (v: string) => void): Mounted {
  const model = monaco.editor.createModel(value, LANGUAGE_ID);
  model.updateOptions({ tabSize: 4, insertSpaces: true });
  const editor = monaco.editor.create(el, {
    model,
    theme: THEME,
    readOnly,
    automaticLayout: true,
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    fontSize: 12,
    lineHeight: 16,
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
    renderLineHighlight: "line",
    fixedOverflowWidgets: true,
    "semanticHighlighting.enabled": true,
    padding: { top: 6, bottom: 6 },
    ariaLabel: "Script source",
  });
  let applying = false;
  let locked = readOnly;
  const sub = model.onDidChangeContent(() => { if (!applying) onChange(model.getValue()); });
  return {
    setValue(v) {
      if (v === model.getValue()) return;
      applying = true;
      try {
        model.pushEditOperations([], [{ range: model.getFullModelRange(), text: v }], () => null);
      } finally {
        applying = false;
      }
    },
    setReadOnly(ro) { locked = ro; editor.updateOptions({ readOnly: ro }); },
    setProblems(ps) {
      const src = model.getValue();
      const markers = ps.flatMap((p) => {
        const m = problemMarker(src, p);
        return m ? [{ ...m, severity: monaco.MarkerSeverity.Error, source: "check" }] : [];
      });
      monaco.editor.setModelMarkers(model, OWNER, markers);
    },
    reveal(p) {
      const m = problemMarker(model.getValue(), p);
      if (!m) return;
      editor.setSelection(new monaco.Selection(m.startLineNumber, m.startColumn, m.endLineNumber, m.endColumn));
      editor.revealRangeInCenterIfOutsideViewport(m);
      editor.focus();
    },
    insert(snippet) {
      if (locked) return;
      editor.focus();
      const c = editor.getContribution("snippetController2") as unknown as SnippetController | null;
      if (c && typeof c.insert === "function") { c.insert(snippet); return; }
      const plain = snippet.replace(/\$\{\d+:([^}]*)\}/g, "$1").replace(/\$\d+/g, "");
      const sel = editor.getSelection();
      if (sel) editor.executeEdits("insert", [{ range: sel, text: plain, forceMoveMarkers: true }]);
    },
    focus() { editor.focus(); },
    dispose() {
      sub.dispose();
      monaco.editor.setModelMarkers(model, OWNER, []);
      editor.dispose();
      model.dispose();
      ide.abortAll();
    },
  };
}
