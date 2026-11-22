// The workbench editor's pure half (docs/game/rules-engine.md, "Workbench"):
// the `h2rules` language's word lists and Monarch grammar, the IDE route's
// answers (`POST /v1/rules/ide`, LSP field names) read defensively, the
// position and range conversions between LSP (0-based, UTF-16) and Monaco
// (1-based), check errors (1-based, byte columns) as marker ranges, the
// semantic token delta encoding, the request client with its debounce,
// cancellation and per-version cache, and the Insert menus. Nothing here
// imports Monaco: stats/rulesMonaco.ts is the thin half that does.

import type { Problem } from "./rules";

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const int = (v: unknown, dflt = 0): number => (typeof v === "number" && Number.isFinite(v) ? Math.floor(v) : dflt);

export const LANGUAGE_ID = "h2rules";

// ---------- the language's words (crates/formats/rules: lang/parse.rs, lang/lower.rs, caps.rs) ----------

/** Declaration and statement words the parser reserves. */
export const KEYWORDS = [
  "rules", "carrier", "teams", "list", "requires", "option", "palette", "enum", "global", "var", "table", "fn", "on",
  "labelled", "text", "budget",
] as const;
export const CONTROL = ["if", "else", "for", "each", "where", "let"] as const;
export const WORD_OPS = ["and", "or", "not"] as const;
export const CONSTANTS = ["true", "false", "none"] as const;
/** Type names (lang/lower.rs). `player`, `team` and `object` are also scopes. */
export const TYPES = ["int", "fixed", "bool", "player", "team", "object", "squad", "timer", "class", "tag", "label", "text"] as const;
/** The finite sets `for each` walks. */
export const SETS = ["players", "teams", "objects", "objectives"] as const;
/** Carrier engine names (`caps::CARRIERS`). */
export const CARRIERS = ["ctf", "slayer", "oddball", "king", "juggernaut", "territories", "assault"] as const;
/** `variant.<field>` names (`caps::VARIANT_FIELDS`). */
export const VARIANT_FIELDS = [
  "team_play", "score_limit", "time_limit", "lives", "respawn_time", "friendly_fire", "rounds", "engine",
  "oddball_ball_count", "king_moving_hill", "ctf_flag_type", "ctf_reset_time", "ctf_touch_return",
  "ctf_flag_at_home", "assault_arm_time", "territories_count", "territories_contest_time",
] as const;

/** What a capability lets a script do, in the lobby's words (`caps::CAPABILITIES`). */
export const CAP_WORDS: Readonly<Record<string, string>> = {
  "game.traits": "changes gravity, damage, shields and speed",
  "player.traits": "changes gravity, damage, shields and speed",
  "player.team": "moves players between teams",
  "player.spawn": "chooses where players spawn",
  "player.weapons": "gives and takes weapons",
  "player.disguise": "hides or changes a player's body",
  "player.camera": "changes the camera",
  "objects.create": "makes objects",
  "objects.delete": "removes objects",
  "objects.move": "moves objects",
  "ai.squads": "places and commands AI",
  "hud.message": "shows messages",
  "hud.superbounce": "shows the superbounce guide: a priming HUD, marks over the map and bounce medals",
  "forge.move": "lets players move objects during the match",
  "forge.delete": "lets players remove objects during the match",
  "forge.place": "lets players place objects during the match",
  "forge.scale": "lets players resize objects during the match",
};

export type ApiKind = "read" | "action" | "builtin";
export interface ApiEntry {
  name: string;
  /** Argument names as the snippet fills them (the type where no better name). */
  args: string[];
  result: string | null;
  kind: ApiKind;
  /** The capability its script must declare; null is core. */
  cap: string | null;
  /** A snippet other than `name(args)`. */
  snippet?: string;
}

const read = (name: string, args: string[], result: string): ApiEntry => ({ name, args, result, kind: "read", cap: null });
const act = (name: string, args: string[], cap: string | null = null): ApiEntry => ({ name, args, result: null, kind: "action", cap });
const vm = (name: string, args: string[], result: string | null, snippet?: string): ApiEntry =>
  (snippet ? { name, args, result, kind: "builtin", cap: null, snippet } : { name, args, result, kind: "builtin", cap: null });

/** The engine API (`caps::ENTRIES`) and the VM built-ins the compiler
 *  knows (lang/lower.rs), the static half of the Insert menus. */
export const API_ENTRIES: readonly ApiEntry[] = [
  read("player_score", ["p"], "int"),
  read("team_score", ["t"], "int"),
  read("kills", ["p"], "int"),
  read("deaths", ["p"], "int"),
  read("alive", ["p"], "bool"),
  read("team_of", ["p"], "team"),
  read("lives", ["p"], "int"),
  read("round_time_left", [], "int"),
  read("is_objective", ["o"], "bool"),
  read("carrier_of", ["o"], "player"),
  read("object_team", ["o"], "team"),
  read("player_of", ["o"], "player"),
  read("inside", ["p", "zone"], "bool"),
  read("label_number", ["o"], "int"),
  read("label_of", ["o"], "label"),
  read("zone_id", ["o"], "int"),
  read("squad_alive", ["s"], "int"),
  act("score", ["p", "n"]),
  act("end_round", ["winner"]),
  act("set_lives", ["p", "n"]),
  act("respawn", ["p"]),
  act("register_objective", ["o", "t"]),
  act("spawn_objective", ["tag", "at", "t"]),
  act("reset_objective", ["o"]),
  act("role_traits", ["p", "on"]),
  act("set_gravity", ["g"], "game.traits"),
  act("set_team", ["p", "t"], "player.team"),
  act("set_loadout", ["p", "primary", "secondary"], "player.weapons"),
  act("give", ["p", "tag"], "player.weapons"),
  act("take", ["p", "weapon"], "player.weapons"),
  act("spawn_areas", ["t", "label", "n"], "player.spawn"),
  act("create", ["tag", "at"], "objects.create"),
  act("delete", ["o"], "objects.delete"),
  act("place_squad", ["s", "class", "n", "label"], "ai.squads"),
  act("hunt", ["s"], "ai.squads"),
  act("erase_squad", ["s"], "ai.squads"),
  act("message", ["to", "text"], "hud.message"),
  act("forge_move", ["o", "at"], "forge.move"),
  act("forge_delete", ["o"], "forge.delete"),
  act("forge_place", ["tag", "at"], "forge.place"),
  act("forge_scale", ["o", "s"], "forge.scale"),
  vm("count", ["set"], "int", "count(${1:players} ${2:p} where ${3:p.alive})"),
  vm("ticks_held", ["zone", "t"], "int"),
  vm("first", ["set"], null, "first(${1:players} ${2:p} where ${3:p.alive})"),
  vm("pick", ["set", "n"], null, "pick(${1:players}, ${2:1})"),
  vm("len", ["table"], "int"),
  vm("clamp", ["v", "lo", "hi"], "int"),
  vm("min", ["a", "b"], "int"),
  vm("max", ["a", "b"], "int"),
  vm("seconds", ["n"], "int"),
  vm("squad", ["n"], "squad"),
  vm("grant", ["p", "cap", "budget n", "palette \"x\""], null, "grant(${1:p}, ${2:forge.move}, budget ${3:10}, palette \"${4:name}\")"),
  vm("revoke", ["p", "cap"], null, "revoke(${1:p}, ${2:forge.move})"),
  vm("granted", ["p", "cap"], "bool", "granted(${1:p}, ${2:forge.move})"),
];

export interface EventDef { name: string; params: string[]; words: string }

/** The handlers a script may declare (rules-engine.md, "The language"). */
export const EVENTS: readonly EventDef[] = [
  { name: "init", params: [], words: "once, when the script loads" },
  { name: "round_start", params: [], words: "at the start of each round" },
  { name: "tick", params: [], words: "every tick, 30 a second" },
  { name: "kill", params: ["victim", "killer"], words: "a player dies; killer is none for the world" },
  { name: "death", params: ["p"], words: "a player dies" },
  { name: "spawn", params: ["p"], words: "a player spawns" },
  { name: "pickup", params: ["p", "o"], words: "a player picks an object up" },
  { name: "drop", params: ["p", "o"], words: "a player drops an object" },
  { name: "enter", params: ["zone", "o"], words: "an object enters a labelled zone" },
  { name: "leave", params: ["zone", "o"], words: "an object leaves a labelled zone" },
  { name: "press", params: ["p", "o"], words: "a player uses an object" },
  { name: "timer", params: ["t"], words: "a timer variable reaches zero" },
  { name: "squad_dead", params: ["s"], words: "an AI squad's last actor dies" },
  { name: "actor_kill", params: ["killer", "class"], words: "a player kills an AI actor" },
  { name: "bounce", params: ["p", "spot"], words: "a player superbounces" },
  { name: "edit", params: ["p", "o"], words: "a player used a granted power" },
  { name: "join", params: ["p"], words: "a player joins" },
  { name: "quit", params: ["p"], words: "a player leaves the game" },
  { name: "round_end", params: [], words: "at the end of each round" },
];

/** Every name the Monarch grammar shows as a built-in call. */
export const BUILTIN_NAMES: readonly string[] = API_ENTRIES.map((e) => e.name);

// ---------- the Monarch grammar and language configuration ----------

/** The `h2rules` Monarch tokenizer, as plain data (rulesMonaco.ts casts it
 *  to Monaco's type). Mirrors lang/lex.rs: `//` comments, identifiers,
 *  integers, 16.16 fixed literals, strings with \" \\ \n escapes and the
 *  23 punctuators. */
export const MONARCH = {
  defaultToken: "invalid",
  tokenPostfix: ".h2rules",
  keywords: [...KEYWORDS],
  control: [...CONTROL],
  wordOps: [...WORD_OPS],
  constants: [...CONSTANTS],
  types: [...TYPES],
  sets: [...SETS],
  builtins: [...BUILTIN_NAMES],
  brackets: [
    { open: "{", close: "}", token: "delimiter.curly" },
    { open: "[", close: "]", token: "delimiter.square" },
    { open: "(", close: ")", token: "delimiter.parenthesis" },
  ],
  tokenizer: {
    root: [
      [/\/\/.*$/, "comment"],
      [/(on)(\s+)([A-Za-z_]\w*)/, ["keyword", "", "type.event"]],
      [/(fn)(\s+)([A-Za-z_]\w*)/, ["keyword", "", "entity.name.function"]],
      [/(global|player|team|object)(\s+)(var)\b/, ["keyword", "", "keyword"]],
      [/(for)(\s+)(each)\b/, ["keyword.control", "", "keyword.control"]],
      [/(team|text)(\s+)(\d+)/, ["keyword", "", "number"]],
      [/(variant)(\.)([A-Za-z_]\w*)/, ["variable.predefined", "delimiter", "variable.predefined"]],
      [/[A-Za-z_]\w*/, {
        cases: {
          "@keywords": "keyword",
          "@control": "keyword.control",
          "@wordOps": "keyword.operator",
          "@constants": "constant",
          "@types": "type",
          "@sets": "variable.predefined",
          "@builtins": "predefined",
          "@default": "identifier",
        },
      }],
      [/\d+\.\d+/, "number.float"],
      [/\d+/, "number"],
      [/"/, "string", "@string"],
      [/[{}()[\]]/, "@brackets"],
      [/\.\.|==|!=|<=|>=|[=<>+\-*\/%]/, "operator"],
      [/[,:;.]/, "delimiter"],
      [/\s+/, ""],
    ],
    string: [
      [/[^\\"]+/, "string"],
      [/\\["\\n]/, "string.escape"],
      [/\\./, "string.escape.invalid"],
      [/"/, "string", "@pop"],
    ],
  },
};

/** Comments, brackets, auto-closing pairs, indentation and folding. */
export const LANGUAGE_CONFIG = {
  comments: { lineComment: "//" },
  brackets: [["{", "}"], ["[", "]"], ["(", ")"]] as Array<[string, string]>,
  autoClosingPairs: [
    { open: "{", close: "}" },
    { open: "[", close: "]" },
    { open: "(", close: ")" },
    { open: "\"", close: "\"", notIn: ["string", "comment"] },
  ],
  surroundingPairs: [
    { open: "{", close: "}" },
    { open: "[", close: "]" },
    { open: "(", close: ")" },
    { open: "\"", close: "\"" },
  ],
  indentationRules: {
    increaseIndentPattern: /^.*(\{[^}"]*|\[[^\]"]*)$/,
    decreaseIndentPattern: /^\s*[}\]]/,
  },
  folding: {
    markers: { start: /^\s*\/\/\s*#region\b/, end: /^\s*\/\/\s*#endregion\b/ },
  },
  wordPattern: /[A-Za-z_]\w*|\d+(?:\.\d+)?/,
};

// ---------- positions and ranges ----------

export interface LspPosition { line: number; character: number }
export interface LspRange { start: LspPosition; end: LspPosition }
/** Monaco's IRange, structurally. */
export interface EditorRange { startLineNumber: number; startColumn: number; endLineNumber: number; endColumn: number }

/** Monaco's 1-based line and column to the route's 0-based line and UTF-16
 *  character. Monaco's columns already count UTF-16 code units. */
export const toLsp = (lineNumber: number, column: number): LspPosition =>
  ({ line: Math.max(0, lineNumber - 1), character: Math.max(0, column - 1) });

export const fromLspRange = (r: LspRange): EditorRange => ({
  startLineNumber: r.start.line + 1,
  startColumn: r.start.character + 1,
  endLineNumber: r.end.line + 1,
  endColumn: r.end.character + 1,
});

function parsePos(v: unknown): LspPosition | null {
  const p = obj(v);
  return typeof p.line === "number" && typeof p.character === "number" ? { line: Math.max(0, int(p.line)), character: Math.max(0, int(p.character)) } : null;
}

export function parseRange(v: unknown): LspRange | null {
  const r = obj(v);
  const start = parsePos(r.start), end = parsePos(r.end);
  return start && end ? { start, end } : null;
}

/** A check error's column (from 1, counting UTF-8 bytes, as lang/lex.rs
 *  does) as a Monaco column (from 1, counting UTF-16 units), clamped to the
 *  line's end. */
export function byteColToUtf16(lineText: string, byteCol: number): number {
  const want = Math.max(0, byteCol - 1);
  let bytes = 0, units = 0;
  for (const ch of lineText) {
    if (bytes >= want) break;
    const cp = ch.codePointAt(0) ?? 0;
    bytes += cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
    units += ch.length;
  }
  return units + 1;
}

export interface Marker extends EditorRange { message: string }

/** A check error as a marker: from its column over the word there (one
 *  character when there is no word). Whole-script errors (line 0) have no
 *  place in the text and stay in the list only. */
export function problemMarker(src: string, p: Problem): Marker | null {
  if (p.line <= 0) return null;
  const lines = src.split("\n");
  const line = Math.min(p.line, Math.max(1, lines.length));
  const text = (lines[line - 1] ?? "").replace(/\r$/, "");
  const start = byteColToUtf16(text, Math.max(1, p.col));
  let end = start - 1;
  while (end < text.length && /\w/.test(text[end] ?? "")) end++;
  const endColumn = end > start - 1 ? end + 1 : Math.min(text.length + 1, start + 1);
  return { startLineNumber: line, startColumn: start, endLineNumber: line, endColumn: Math.max(start, endColumn), message: p.message };
}

// ---------- the route's answers ----------

export type IdeOp = "complete" | "hover" | "signature" | "definition" | "references" | "tokens" | "symbols" | "format";

/** LSP's CompletionItemKind, 1 to 25, by name; every name is also a key of
 *  Monaco's `languages.CompletionItemKind`. */
export const LSP_COMPLETION_KINDS = [
  "Text", "Method", "Function", "Constructor", "Field", "Variable", "Class", "Interface", "Module", "Property", "Unit",
  "Value", "Enum", "Keyword", "Snippet", "Color", "File", "Reference", "Folder", "EnumMember", "Constant", "Struct",
  "Event", "Operator", "TypeParameter",
] as const;
export type CompletionKindName = (typeof LSP_COMPLETION_KINDS)[number];
export const completionKindName = (k: number): CompletionKindName => LSP_COMPLETION_KINDS[k - 1] ?? "Text";

/** LSP's SymbolKind, 1 to 26, by name; every name is also a key of Monaco's
 *  `languages.SymbolKind`. */
export const LSP_SYMBOL_KINDS = [
  "File", "Module", "Namespace", "Package", "Class", "Method", "Property", "Field", "Constructor", "Enum", "Interface",
  "Function", "Variable", "Constant", "String", "Number", "Boolean", "Array", "Object", "Key", "Null", "EnumMember",
  "Struct", "Event", "Operator", "TypeParameter",
] as const;
export type SymbolKindName = (typeof LSP_SYMBOL_KINDS)[number];
export const symbolKindName = (k: number): SymbolKindName => LSP_SYMBOL_KINDS[k - 1] ?? "Variable";

/** LSP completion kinds this page reads. */
export const CK = { Function: 3, Variable: 6, Keyword: 14, Snippet: 15, Constant: 21, Event: 23, Property: 10, Type: 25 } as const;

/** LSP MarkupContent, a MarkedString, or a list of them, as markdown. */
export function markdownOf(v: unknown): string {
  if (typeof v === "string") return v;
  if (Array.isArray(v)) return v.map(markdownOf).filter(Boolean).join("\n\n");
  const m = obj(v);
  if (typeof m.value !== "string") return "";
  return typeof m.language === "string" ? "```" + m.language + "\n" + m.value + "\n```" : m.value;
}

export interface IdeCompletion {
  label: string;
  kind: number;
  detail: string;
  documentation: string;
  insertText: string;
  snippet: boolean;
}

export function parseCompletions(body: unknown): IdeCompletion[] {
  const list = Array.isArray(body) ? body : arr(obj(body).items);
  return list.map(obj).filter((c) => typeof c.label === "string" && c.label).map((c) => ({
    label: str(c.label),
    kind: int(c.kind, 1),
    detail: str(c.detail),
    documentation: markdownOf(c.documentation),
    insertText: str(c.insertText) || str(c.label),
    snippet: c.insertTextFormat === 2,
  }));
}

export interface IdeHover { contents: string; range: LspRange | null }
export function parseHover(body: unknown): IdeHover | null {
  const b = obj(body);
  const contents = markdownOf(b.contents);
  return contents ? { contents, range: parseRange(b.range) } : null;
}

export interface IdeSignature { label: string; documentation: string; parameters: string[] }
export interface IdeSignatureHelp { signatures: IdeSignature[]; activeSignature: number; activeParameter: number }
export function parseSignature(body: unknown): IdeSignatureHelp | null {
  const b = obj(body);
  const signatures = arr(b.signatures).map(obj).filter((s) => typeof s.label === "string").map((s) => ({
    label: str(s.label),
    documentation: markdownOf(s.documentation),
    parameters: arr(s.parameters).map(obj).map((p) => (typeof p.label === "string" ? p.label : Array.isArray(p.label) ? str(s.label).slice(int(p.label[0]), int(p.label[1])) : "")),
  }));
  if (!signatures.length) return null;
  return {
    signatures,
    activeSignature: Math.min(signatures.length - 1, Math.max(0, int(b.activeSignature))),
    activeParameter: Math.max(0, int(b.activeParameter)),
  };
}

/** Definition and references: `[{range}]` (a lone location is read too). */
export function parseLocations(body: unknown): LspRange[] {
  const list = Array.isArray(body) ? body : body && typeof body === "object" ? [body] : [];
  return list.map((l) => parseRange(obj(l).range)).filter((r): r is LspRange => r !== null);
}

export interface SemToken { line: number; start: number; length: number; type: number | string }
export interface IdeTokens { legend: string[] | null; data: SemToken[] }
export function parseTokens(body: unknown): IdeTokens | null {
  const b = obj(body);
  if (!Array.isArray(b.data)) return null;
  const legend = Array.isArray(b.legend) ? b.legend.filter((x): x is string => typeof x === "string") : null;
  const data = b.data.map(obj).map((t) => ({
    line: int(t.line, -1),
    start: int(t.start, -1),
    length: int(t.length),
    type: typeof t.type === "string" ? t.type : int(t.type, -1),
  })).filter((t) => t.line >= 0 && t.start >= 0 && t.length > 0);
  return { legend, data };
}

/** `GET /v1/rules/ide/legend`: a list, or `{legend}`, or `{tokenTypes}`. */
export function parseLegend(body: unknown): string[] | null {
  const b = obj(body);
  const list = Array.isArray(body) ? body : Array.isArray(b.legend) ? b.legend : Array.isArray(b.tokenTypes) ? b.tokenTypes : null;
  const names = list?.filter((x): x is string => typeof x === "string") ?? [];
  return names.length ? names : null;
}

/** The legend used until the route's arrives, or when it can't. */
export const DEFAULT_LEGEND: readonly string[] = [
  "keyword", "function", "variable", "parameter", "property", "type", "enumMember", "event", "string", "number",
  "comment", "operator", "namespace",
];

/** Tokens as Monaco's semantic token data: five numbers a token (line
 *  delta, start delta on the same line or absolute on a new one, length,
 *  type index into `legend`, no modifiers), in document order. A type the
 *  legend lacks is dropped. `theirs` is the answer's own legend when it
 *  names types by index into it. */
export function encodeTokens(tokens: readonly SemToken[], legend: readonly string[], theirs: readonly string[] | null = null): number[] {
  const index = (t: number | string): number => {
    if (typeof t === "string") return legend.indexOf(t);
    if (theirs) { const name = theirs[t]; return name === undefined ? -1 : legend.indexOf(name); }
    return t >= 0 && t < legend.length ? t : -1;
  };
  const sorted = tokens
    .map((t) => ({ ...t, ix: index(t.type) }))
    .filter((t) => t.ix >= 0 && t.length > 0)
    .sort((a, b) => a.line - b.line || a.start - b.start);
  const out: number[] = [];
  let line = 0, start = 0;
  for (const t of sorted) {
    const dl = t.line - line;
    out.push(dl, dl === 0 ? t.start - start : t.start, t.length, t.ix, 0);
    line = t.line;
    start = t.start;
  }
  return out;
}

export interface IdeSymbol { name: string; kind: number; range: LspRange; selectionRange: LspRange; children: IdeSymbol[] }
export function parseSymbols(body: unknown): IdeSymbol[] {
  const list = Array.isArray(body) ? body : arr(obj(body).items);
  return list.map(obj).flatMap((s): IdeSymbol[] => {
    const range = parseRange(s.range);
    if (!range || typeof s.name !== "string") return [];
    return [{ name: s.name, kind: int(s.kind, 13), range, selectionRange: parseRange(s.selectionRange) ?? range, children: parseSymbols(s.children) }];
  });
}

export type IdeFormat = { text: string } | { error: string };
export function parseFormat(body: unknown): IdeFormat | null {
  const b = obj(body);
  if (typeof b.text === "string") return { text: b.text };
  if (typeof b.error === "string") return { error: b.error };
  return null;
}

// ---------- the client ----------

export interface IdeRequest { source: string; op: IdeOp; line: number; character: number }

/** How long each op waits for typing to pause before it asks. */
export const IDE_DELAY_MS: Readonly<Record<IdeOp, number>> = {
  complete: 40, hover: 60, signature: 40, definition: 0, references: 0, tokens: 200, symbols: 250, format: 0,
};

const DOCUMENT_OPS: ReadonlySet<IdeOp> = new Set(["tokens", "symbols", "format"]);

function sleep(ms: number, signal: AbortSignal): Promise<boolean> {
  return new Promise((resolve) => {
    if (signal.aborted) { resolve(false); return; }
    if (ms <= 0) { resolve(true); return; }
    const t = setTimeout(() => resolve(!signal.aborted), ms);
    signal.addEventListener("abort", () => { clearTimeout(t); resolve(false); }, { once: true });
  });
}

/** Asks `POST /v1/rules/ide`. A newer request of an op cancels the older
 *  one, each waits its op's delay first, and answers are kept per model
 *  version (document ops by version alone, the rest by position too). An
 *  abandoned, failed or refused request answers null. */
export class IdeClient {
  private cache = new Map<string, unknown>();
  private version = Number.NaN;
  private inflight = new Map<IdeOp, AbortController>();

  constructor(
    private api: () => string,
    private fetchFn: typeof fetch = fetch,
    private delays: Readonly<Record<IdeOp, number>> = IDE_DELAY_MS,
  ) {}

  async request(op: IdeOp, source: string, version: number, pos: LspPosition, signal?: AbortSignal): Promise<unknown> {
    if (version !== this.version) { this.cache.clear(); this.version = version; }
    const key = DOCUMENT_OPS.has(op) ? op : `${op}:${pos.line}:${pos.character}`;
    if (this.cache.has(key)) return this.cache.get(key);
    this.inflight.get(op)?.abort();
    const ctl = new AbortController();
    this.inflight.set(op, ctl);
    if (signal) {
      if (signal.aborted) ctl.abort();
      else signal.addEventListener("abort", () => ctl.abort(), { once: true });
    }
    try {
      if (!(await sleep(this.delays[op], ctl.signal))) return null;
      const body: IdeRequest = { source, op, line: pos.line, character: pos.character };
      const f = this.fetchFn; // called unbound: fetch refuses another `this`
      const r = await f(`${this.api()}/rules/ide`, {
        method: "POST", signal: ctl.signal, headers: { accept: "application/json", "content-type": "application/json" }, body: JSON.stringify(body),
      });
      if (!r.ok) return null;
      const answer: unknown = await r.json();
      if (ctl.signal.aborted) return null;
      if (version === this.version) this.cache.set(key, answer);
      return answer;
    } catch {
      return null;
    } finally {
      if (this.inflight.get(op) === ctl) this.inflight.delete(op);
    }
  }

  /** Drops every request in flight (the editor is going away). */
  abortAll(): void {
    for (const c of this.inflight.values()) c.abort();
    this.inflight.clear();
  }
}

export async function fetchLegend(api: string, fetchFn: typeof fetch = fetch): Promise<string[] | null> {
  try {
    const r = await fetchFn(`${api}/rules/ide/legend`, { headers: { accept: "application/json" } });
    return r.ok ? parseLegend(await r.json()) : null;
  } catch {
    return null;
  }
}

// ---------- the Insert menus ----------

export interface InsertItem { label: string; detail: string; words: string; snippet: string }
export interface InsertGroup { title: string; words: string; items: InsertItem[] }
export type InsertMenuId = "declarations" | "events" | "api" | "control";
export interface InsertMenu { id: InsertMenuId; label: string; groups: InsertGroup[] }

/** `${n:text}`, a snippet placeholder. */
const ph = (n: number, text: string): string => "${" + n + ":" + text + "}";

/** A call's snippet: each argument a placeholder. */
export const apiSnippet = (e: ApiEntry): string => e.snippet ?? `${e.name}(${e.args.map((a, i) => ph(i + 1, a)).join(", ")})`;
/** A call's signature as the menus and hovers show it. */
export const apiSignature = (e: ApiEntry): string => `${e.name}(${e.args.join(", ")})${e.result ? ` -> ${e.result}` : ""}`;

export const eventLabelOf = (e: EventDef): string => `on ${e.name}${e.params.length ? `(${e.params.join(", ")})` : ""}`;
export function eventSnippet(e: EventDef): string {
  const params = e.params.length ? `(${e.params.map((p, i) => ph(i + 1, p)).join(", ")})` : "";
  return `on ${e.name}${params} {\n\t$0\n}`;
}

export const DECLARATIONS: readonly InsertItem[] = [
  { label: "requires", detail: "requires cap, cap", words: "the capabilities the script asks for", snippet: `requires ${ph(1, "player.weapons")}` },
  { label: "teams", detail: "teams n", words: "how many teams the game type has", snippet: `teams ${ph(1, "2")}` },
  { label: "list", detail: "list tweakers", words: "the game type list row its variants are filed under", snippet: "list tweakers" },
  { label: "option", detail: "option name: int lo..hi = default \"Label\" \"Description\"", words: "a row the variant editor shows", snippet: `option ${ph(1, "name")}: ${ph(2, "int")} ${ph(3, "1..5")} = ${ph(4, "1")} "${ph(5, "Label")}" "${ph(6, "Description")}"` },
  { label: "option (on or off)", detail: "option name: bool = true \"Label\" \"Description\"", words: "a switch the variant editor shows", snippet: `option ${ph(1, "name")}: bool = ${ph(2, "true")} "${ph(3, "Label")}" "${ph(4, "Description")}"` },
  { label: "global var", detail: "global var name: type = init", words: "one value for the whole game (64 slots)", snippet: `global var ${ph(1, "name")}: ${ph(2, "int")} = ${ph(3, "0")}` },
  { label: "global timer", detail: "global var name: timer", words: "counts ticks down, then runs on timer", snippet: `global var ${ph(1, "clock")}: timer` },
  { label: "player var", detail: "player var name: type = init", words: "a value each player carries (16 slots)", snippet: `player var ${ph(1, "name")}: ${ph(2, "int")} = ${ph(3, "0")}` },
  { label: "team var", detail: "team var name: type = init", words: "a value each team carries (8 slots)", snippet: `team var ${ph(1, "name")}: ${ph(2, "int")} = ${ph(3, "0")}` },
  { label: "object var", detail: "object var name: type = init", words: "a value each labelled object carries (8 slots)", snippet: `object var ${ph(1, "name")}: ${ph(2, "int")} = ${ph(3, "0")}` },
  { label: "enum", detail: "enum Name { a, b }", words: "a type with named values", snippet: `enum ${ph(1, "Name")} { ${ph(2, "first")}, ${ph(3, "second")} }` },
  { label: "table", detail: "table name: (col: type) = [ (value) ]", words: "constant rows, indexed by int or by the first column", snippet: `table ${ph(1, "name")}: (${ph(2, "weapon")}: ${ph(3, "tag")}) = [\n\t(${ph(4, "\"objects/weapons/rifle/smg/smg\"")}),\n]` },
  { label: "palette", detail: "palette \"name\" = [\"tag\", ...]", words: "what a granted Forge power may place", snippet: `palette "${ph(1, "name")}" = [${ph(2, "\"objects/forge/block_wall/block_wall\"")}]` },
  { label: "fn", detail: "fn name(p: player) { ... }", words: "a function, inlined where it is called", snippet: `fn ${ph(1, "name")}(${ph(2, "p")}: ${ph(3, "player")}) {\n\t$0\n}` },
];

export const CONTROL_ITEMS: readonly InsertItem[] = [
  { label: "if", detail: "if cond { ... }", words: "run a block when something is true", snippet: `if ${ph(1, "cond")} {\n\t$0\n}` },
  { label: "if / else", detail: "if cond { ... } else { ... }", words: "one block or the other", snippet: `if ${ph(1, "cond")} {\n\t${ph(2, "")}\n} else {\n\t$0\n}` },
  { label: "else if", detail: "else if cond { ... }", words: "another test after an if", snippet: `else if ${ph(1, "cond")} {\n\t$0\n}` },
  { label: "for each player", detail: "for each players p where cond { ... }", words: "every player, or those that match", snippet: `for each players ${ph(1, "p")} where ${ph(2, "p.alive")} {\n\t$0\n}` },
  { label: "for each team", detail: "for each teams t { ... }", words: "every team", snippet: `for each teams ${ph(1, "t")} {\n\t$0\n}` },
  { label: "for each labelled object", detail: "for each objects labelled \"x\" o { ... }", words: "every object with a Forge label", snippet: `for each objects labelled "${ph(1, "label")}" ${ph(2, "o")} {\n\t$0\n}` },
  { label: "for each objective", detail: "for each objectives o { ... }", words: "the game's objectives (flags, balls, bombs)", snippet: `for each objectives ${ph(1, "o")} {\n\t$0\n}` },
  { label: "let", detail: "let name = value", words: "a name for a value, not assigned again", snippet: `let ${ph(1, "name")} = ${ph(2, "value")}` },
];

export function apiGroups(entries: readonly ApiEntry[]): InsertGroup[] {
  const item = (e: ApiEntry): InsertItem => ({
    label: e.name,
    detail: apiSignature(e),
    words: e.cap ? `needs requires ${e.cap}` : e.kind === "read" ? "a read" : e.kind === "builtin" ? "built into the VM" : "core",
    snippet: apiSnippet(e),
  });
  const groups: InsertGroup[] = [
    { title: "Reads", words: "reads the game; needs no capability", items: entries.filter((e) => e.kind === "read").map(item) },
    { title: "Core actions", words: "scores, rounds, lives and objectives; needs no capability", items: entries.filter((e) => e.kind === "action" && !e.cap).map(item) },
  ];
  const caps = [...new Set(entries.filter((e) => e.cap).map((e) => e.cap as string))];
  for (const cap of caps) {
    groups.push({ title: cap, words: CAP_WORDS[cap] ?? "an unknown capability", items: entries.filter((e) => e.cap === cap).map(item) });
  }
  groups.push({ title: "Built-ins", words: "sets, tables, numbers and powers", items: entries.filter((e) => e.kind === "builtin").map(item) });
  return groups.filter((g) => g.items.length);
}

const IDENT = /^[A-Za-z_]\w*$/;

/** The completion route's functions merged over the static list: a known
 *  name takes the route's detail and snippet, an unknown one is added to
 *  its own group. */
export function mergeApi(items: readonly IdeCompletion[] | null): { entries: ApiEntry[]; extra: InsertItem[] } {
  const entries = API_ENTRIES.map((e) => ({ ...e }));
  const extra: InsertItem[] = [];
  for (const c of items ?? []) {
    if (c.kind !== CK.Function && c.kind !== 2) continue;
    const name = c.label.replace(/\(.*$/, "").trim();
    if (!IDENT.test(name)) continue;
    const known = entries.find((e) => e.name === name);
    if (known) {
      if (c.snippet) known.snippet = c.insertText;
      continue;
    }
    extra.push({ label: name, detail: c.detail || name, words: c.documentation.split("\n")[0] ?? "", snippet: c.snippet ? c.insertText : `${name}($0)` });
  }
  return { entries, extra };
}

/** The completion route's handlers merged over the static event list. */
export function mergeEvents(items: readonly IdeCompletion[] | null): InsertItem[] {
  const out: InsertItem[] = EVENTS.map((e) => ({ label: eventLabelOf(e), detail: eventLabelOf(e), words: e.words, snippet: eventSnippet(e) }));
  for (const c of items ?? []) {
    const m = /^(?:on\s+)?([A-Za-z_]\w*)/.exec(c.label);
    if (!m || !(c.kind === CK.Event || /^on\s/.test(c.label))) continue;
    const name = m[1] ?? "";
    const at = out.findIndex((i) => i.label === `on ${name}` || i.label.startsWith(`on ${name}(`));
    const it: InsertItem = {
      label: c.label.startsWith("on ") ? c.label : `on ${c.label}`,
      detail: c.detail || c.label,
      words: c.documentation.split("\n")[0] || (at >= 0 ? out[at]?.words ?? "" : ""),
      snippet: c.snippet ? c.insertText : `on ${name} {\n\t$0\n}`,
    };
    if (at >= 0) out[at] = { ...it, label: out[at]?.label ?? it.label };
    else out.push(it);
  }
  return out;
}

/** The four Insert menus, from the route's completions when it gave any
 *  (`top` at a top-level position, `body` inside a handler). */
export function insertMenus(top: readonly IdeCompletion[] | null, body: readonly IdeCompletion[] | null): InsertMenu[] {
  const { entries, extra } = mergeApi(body);
  const api = apiGroups(entries);
  if (extra.length) api.push({ title: "More", words: "named by the server, not in this page's list", items: extra });
  return [
    { id: "declarations", label: "Declarations", groups: [{ title: "Declarations", words: "the header, options, variables, types and functions", items: [...DECLARATIONS] }] },
    { id: "events", label: "Events", groups: [{ title: "Handlers", words: "what runs when something happens; each runs to completion", items: mergeEvents(top) }] },
    { id: "api", label: "Actions and reads", groups: api },
    { id: "control", label: "Control", groups: [{ title: "Control", words: "tests, loops over finite sets, and names", items: [...CONTROL_ITEMS] }] },
  ];
}

/** Where the menus ask the completion route: a blank top-level line, and a
 *  blank statement inside a handler. */
export const MENU_PROBES: ReadonlyArray<{ which: "top" | "body"; source: string; line: number; character: number }> = [
  { which: "top", source: "rules \"Menu\" 1\ncarrier slayer\n\n", line: 2, character: 0 },
  { which: "body", source: "rules \"Menu\" 1\ncarrier slayer\n\non tick {\n    \n}\n", line: 4, character: 4 },
];

export async function fetchMenuCompletions(api: string, fetchFn: typeof fetch = fetch): Promise<{ top: IdeCompletion[] | null; body: IdeCompletion[] | null }> {
  const ask = async (p: (typeof MENU_PROBES)[number]): Promise<IdeCompletion[] | null> => {
    try {
      const req: IdeRequest = { source: p.source, op: "complete", line: p.line, character: p.character };
      const r = await fetchFn(`${api}/rules/ide`, { method: "POST", headers: { accept: "application/json", "content-type": "application/json" }, body: JSON.stringify(req) });
      if (!r.ok) return null;
      const items = parseCompletions(await r.json());
      return items.length ? items : null;
    } catch {
      return null;
    }
  };
  const [top, body] = await Promise.all(MENU_PROBES.map(ask));
  return { top: top ?? null, body: body ?? null };
}
