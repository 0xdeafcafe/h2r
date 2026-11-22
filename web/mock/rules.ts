// The rules workbench for the mock (docs/live/live-api.md "Rules scripts"),
// so the Game Types page runs with `bun run dev` and no server. The real
// server compiles and verifies with the `rules` crate; this answers in the
// same shapes from a few text checks:
// - `POST /v1/rules/check`: errors for a missing header line and for
//   unbalanced braces (with their line), handlers and capabilities read
//   off the text, and a hash of the source (the server's is of the IR).
// - `POST /v1/rules/run`: each listed event "runs" when the script has an
//   `on <event>` handler, and a kill calls score() as Slayer does.
// - `GET /v1/rules/builtin`: Slayer, read from the rules crate.
// - The check also lists the script's int and bool options and its
//   carrier, as the server's may.
// - `POST /v1/rules` (a `content.write` token), `GET /v1/rules/{hash}`,
//   `GET /v1/rules?author=`: kept in memory.
// - `POST /v1/rules/ide` and `GET /v1/rules/ide/legend`: the editor's
//   completions, hovers, signatures, definitions, references, tokens,
//   outline and format, canned from the page's own word lists
//   (src/lib/rulesIde.ts) and a few regexes (`mockIde`).
// - With the tools off (`H2_MOCK_RULES=0`), every route here is the 404
//   "no such endpoint" of a server from before them, so the page's
//   "no script tools" state can be seen.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { parseJws } from "../src/state/verify";
import type { Reply } from "./admin";
import type { World } from "./world";
import {
  API_ENTRIES, CAP_WORDS, CONSTANTS, CONTROL, EVENTS, KEYWORDS, SETS, TYPES, WORD_OPS, apiSignature, apiSnippet,
  eventLabelOf, eventSnippet, type IdeOp,
} from "../src/lib/rulesIde";

const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });
const MAX_SOURCE = 256 * 1024;
const MAX_TICKS = 36_000;

const WORDS: Record<string, [string, boolean]> = {
  "game.traits": ["changes gravity, damage, shields and speed", false],
  "player.traits": ["changes gravity, damage, shields and speed", false],
  "player.team": ["moves players between teams", false],
  "player.spawn": ["chooses where players spawn", false],
  "player.weapons": ["gives and takes weapons", false],
  "objects.create": ["makes objects", false],
  "objects.delete": ["removes objects", false],
  "ai.squads": ["places and commands AI", false],
  "hud.message": ["shows messages", false],
  "forge.move": ["lets players move objects during the match", true],
  "forge.delete": ["lets players remove objects during the match", true],
  "forge.place": ["lets players place objects during the match", true],
  "forge.scale": ["lets players resize objects during the match", true],
};

const SLAYER_FALLBACK = `rules "Slayer" 1
carrier slayer

option kill_points: int 1..5 = 1 "Kill points" "Points for each kill"
option suicide_costs: bool = true "Suicide penalty" "A suicide costs a point"

on kill(victim, killer) {
    if killer == none or killer == victim {
        if suicide_costs { score(victim, -1) }
    } else if variant.team_play and killer.team == victim.team {
        score(killer, -1)
    } else {
        score(killer, kill_points)
    }
}
`;

function slayer(): string {
  try {
    return readFileSync(new URL("../../crates/formats/rules/src/builtin/slayer.rules", import.meta.url), "utf8");
  } catch {
    return SLAYER_FALLBACK;
  }
}

const sha = (s: string) => createHash("sha256").update(s).digest("hex");

/** The built-ins `GET /v1/rules/builtin` lists (mock/variants.ts reads
 *  them too). */
export function builtinScripts(): Array<{ name: string; engine: number; source: string; hash: string; example: boolean }> {
  const source = slayer();
  return [{ name: "Slayer", engine: 2, source, hash: sha(source), example: false }];
}

const CARRIER_ENGINES: Record<string, number> = { ctf: 1, slayer: 2, oddball: 3, king: 4, juggernaut: 7, territories: 8, assault: 9 };

/** The `option` lines of a script's text, as the check lists them (ints
 *  and bools; the mock reads no enums). */
export function optionsOf(source: string) {
  const out: Array<{ name: string; label: string; description: string | null; kind: "int" | "bool"; lo: number; hi: number; default: number | boolean }> = [];
  for (const m of source.matchAll(/^\s*option\s+([A-Za-z_]\w*)\s*:\s*(?:int\s+(-?\d+)\.\.(-?\d+)|(bool))\s*=\s*(\S+)\s+"([^"]*)"(?:\s+"([^"]*)")?/gm)) {
    const name = m[1]!, label = m[6] || name, description = m[7] ?? null;
    if (m[4]) out.push({ name, label, description, kind: "bool", lo: 0, hi: 1, default: m[5] === "true" });
    else out.push({ name, label, description, kind: "int", lo: Number(m[2]), hi: Number(m[3]), default: Number(m[5]) });
  }
  return out;
}

/** The carrier a script's text names: its keyword and engine index. */
export function carrierOfSource(source: string): { name: string; engine: number } | null {
  const name = /^\s*carrier\s+([a-z_]+)/m.exec(source)?.[1];
  const engine = name ? CARRIER_ENGINES[name] : undefined;
  return name && engine !== undefined ? { name, engine } : null;
}

/** The handlers a script's text declares: `kill`, `timer(next)`. */
export function handlersOf(source: string): string[] {
  return [...source.matchAll(/^\s*on\s+([a-z_]+)(?:\s*\(([^)]*)\))?/gm)].map((m) => (m[1] === "timer" ? `timer(${(m[2] ?? "").trim()})` : m[1]!));
}

export function mockCheck(source: string) {
  const errors: Array<{ line: number; col: number; message: string }> = [];
  const head = /^\s*rules\s+"([^"]*)"\s+(\d+)/.exec(source);
  if (!head) errors.push({ line: 1, col: 1, message: 'a script starts with rules "<name>" <version>' });
  let depth = 0;
  const lines = source.split("\n");
  lines.forEach((l, i) => {
    for (let c = 0; c < l.length; c++) {
      if (l[c] === "{") depth++;
      if (l[c] === "}" && --depth < 0) { errors.push({ line: i + 1, col: c + 1, message: "unexpected }" }); depth = 0; }
    }
  });
  if (depth > 0) errors.push({ line: lines.length, col: 1, message: "a { is never closed" });
  if (errors.length) return { ok: false, hash: null, name: null, version: null, errors, report: null };
  const handlers = handlersOf(source).map((event) => ({ event, worst_ops: 24 }));
  const requires = (/^\s*requires\s+(.+)$/m.exec(source)?.[1] ?? "").split(",").map((s) => s.trim()).filter(Boolean);
  const count = (scope: string) => [...source.matchAll(new RegExp(`^\\s*${scope}\\s+var\\b`, "gm"))].length;
  const per = (e: string) => (["tick", "init", "round_start", "round_end"].includes(e) || e.startsWith("timer(") ? 1 : e === "enter" || e === "leave" ? 64 : 16);
  const worst = handlers.reduce((n, h) => n + h.worst_ops * per(h.event), 0);
  return {
    ok: worst <= 20_000,
    hash: sha(source),
    name: head![1],
    version: Number(head![2]),
    errors: worst <= 20_000 ? [] : [{ line: 0, col: 0, message: `the worst tick costs ${worst} ops, over the budget of 20000` }],
    options: optionsOf(source),
    carrier: carrierOfSource(source),
    report: {
      worst_tick_ops: worst,
      budget: 20_000,
      fits: worst <= 20_000,
      costliest: [...handlers].sort((a, b) => b.worst_ops * per(b.event) - a.worst_ops * per(a.event))[0]?.event ?? null,
      handlers,
      slots: [["global", 64], ["player", 16], ["team", 8], ["object", 8]].map(([scope, limit]) => ({ scope, used: count(scope as string), limit })),
      ir_bytes: Math.min(64 * 1024, 64 + Math.ceil(source.length / 3)),
      ir_limit: 64 * 1024,
      capabilities: requires.map((name) => ({ name, words: WORDS[name]?.[0] ?? "", grantable: WORDS[name]?.[1] ?? false, known: name in WORDS })),
      unhostable: /\bset_team\s*\(/.test(source) ? ["set_team"] : [],
    },
  };
}

interface WireTick { tick: number; events: Array<Record<string, unknown>> }

export function mockRun(source: string, scenario: Record<string, unknown>) {
  const check = mockCheck(source);
  if (!check.ok) return { ok: false, errors: check.errors };
  const timeline = (Array.isArray(scenario.timeline) ? scenario.timeline : []) as WireTick[];
  const last = typeof scenario.ticks === "number" ? scenario.ticks : timeline.reduce((m, t) => Math.max(m, t.tick), 0);
  if (last > MAX_TICKS) return null;
  const handlers = new Set(handlersOf(source));
  const players = (Array.isArray(scenario.players) ? scenario.players : []) as Array<{ slot: number; team?: number }>;
  const scores = new Map(players.map((p) => [p.slot, 0]));
  const alive = new Map(players.map((p) => [p.slot, true]));
  const ticks = [...timeline].sort((a, b) => a.tick - b.tick).filter((t) => t.tick <= last).map((t) => {
    const ran: string[] = [];
    const calls: Array<{ entry: string; args: string[] }> = [];
    for (const e of t.events ?? []) {
      const type = String(e.type);
      if (type === "kill") alive.set(Number(e.victim), false);
      if (type === "spawn") alive.set(Number(e.player), true);
      if (!handlers.has(type)) continue;
      ran.push(type);
      if (type === "kill") {
        const v = Number(e.victim), k = typeof e.killer === "number" ? e.killer : null;
        const who = k === null || k === v ? v : k, pts = k === null || k === v ? -1 : 1;
        scores.set(who, (scores.get(who) ?? 0) + pts);
        calls.push({ entry: "score", args: [`player ${who}`, String(pts)] });
      }
    }
    return {
      tick: t.tick, ran, calls, ops: ran.length * 12, warnings: [], globals: [], error: null,
      players: players.map((p) => ({ slot: p.slot, team: p.team ?? 0, alive: alive.get(p.slot) ?? true, score: scores.get(p.slot) ?? 0, lives: 0, vars: [] })),
    };
  }).filter((t) => t.ran.length || t.calls.length);
  return {
    ok: true, hash: check.hash, options: 0, ticks, last_tick: last, error: null, round_ended: null, cut: null,
    ops: ticks.reduce((n, t) => n + t.ops, 0),
    scores: [...scores.entries()].map(([slot, score]) => ({ slot, score })),
  };
}

// ---------- the editor's IDE route ----------

/** The mock's token legend. */
export const MOCK_LEGEND = ["keyword", "function", "variable", "parameter", "property", "type", "enumMember", "event", "string", "number", "comment", "operator", "namespace"];
const IDE_OPS: ReadonlySet<string> = new Set(["complete", "hover", "signature", "definition", "references", "tokens", "symbols", "format"]);
const RESERVED: ReadonlySet<string> = new Set<string>([...KEYWORDS, ...CONTROL, ...WORD_OPS, ...CONSTANTS]);
const TYPE_NAMES: ReadonlySet<string> = new Set<string>(TYPES);
const SET_NAMES: ReadonlySet<string> = new Set<string>([...SETS, "variant"]);
const API_NAMES: ReadonlySet<string> = new Set(API_ENTRIES.map((e) => e.name));

interface Range { start: { line: number; character: number }; end: { line: number; character: number } }
const span = (line: number, start: number, endLine: number, end: number): Range =>
  ({ start: { line, character: start }, end: { line: endLine, character: end } });

/** The identifier at a position, and where it sits on its line. */
function wordAt(lines: string[], line: number, character: number): { word: string; start: number; end: number } | null {
  const text = lines[line] ?? "";
  for (const m of text.matchAll(/[A-Za-z_]\w*/g)) {
    const start = m.index ?? 0, end = start + m[0].length;
    if (character >= start && character <= end) return { word: m[0], start, end };
  }
  return null;
}

const DECLARES = /^\s*(?:fn|option|enum|table|let|(?:global|player|team|object)\s+var)\s+([A-Za-z_]\w*)/;

/** Where a name is declared: by fn, option, enum, table, let or var. */
function declarationOf(lines: string[], name: string): Range | null {
  for (let i = 0; i < lines.length; i++) {
    const text = lines[i] ?? "";
    const m = DECLARES.exec(text);
    if (m && m[1] === name) {
      const at = m[0].length - name.length;
      return span(i, at, i, at + name.length);
    }
  }
  return null;
}

function hoverText(name: string): string | null {
  const e = API_ENTRIES.find((x) => x.name === name);
  if (e) {
    const what = e.cap ? `Needs \`requires ${e.cap}\`: ${CAP_WORDS[e.cap] ?? ""}.` : e.kind === "read" ? "A read." : e.kind === "builtin" ? "Built into the VM." : "Core: needs no capability.";
    return "```h2rules\n" + apiSignature(e) + "\n```\n" + what;
  }
  const ev = EVENTS.find((x) => x.name === name);
  if (ev) return "```h2rules\n" + eventLabelOf(ev) + "\n```\n" + ev.words;
  if (RESERVED.has(name)) return `\`${name}\`: a keyword.`;
  if (TYPE_NAMES.has(name)) return `\`${name}\`: a type.`;
  return null;
}

function mockTokens(lines: string[]) {
  const data: Array<{ line: number; start: number; length: number; type: string }> = [];
  lines.forEach((text, line) => {
    const cut = text.indexOf("//");
    const code = cut >= 0 ? text.slice(0, cut) : text;
    let afterOn = false;
    for (const m of code.matchAll(/"(?:[^"\\]|\\.)*"|\d+(?:\.\d+)?|[A-Za-z_]\w*/g)) {
      const t = m[0], start = m.index ?? 0;
      const type = t.startsWith("\"") ? "string"
        : /^\d/.test(t) ? "number"
        : afterOn ? "event"
        : RESERVED.has(t) ? "keyword"
        : TYPE_NAMES.has(t) ? "type"
        : API_NAMES.has(t) ? "function"
        : SET_NAMES.has(t) ? "namespace"
        : null;
      afterOn = t === "on";
      if (type) data.push({ line, start, length: t.length, type });
    }
    if (cut >= 0) data.push({ line, start: cut, length: text.length - cut, type: "comment" });
  });
  return { legend: MOCK_LEGEND, data };
}

const SYMBOLS: ReadonlyArray<[RegExp, number]> = [
  [/^\s*on\s+([a-z_]+)/, 24],
  [/^\s*fn\s+([A-Za-z_]\w*)/, 12],
  [/^\s*(?:global|player|team|object)\s+var\s+([A-Za-z_]\w*)/, 13],
  [/^\s*option\s+([A-Za-z_]\w*)/, 7],
  [/^\s*enum\s+([A-Za-z_]\w*)/, 10],
  [/^\s*table\s+([A-Za-z_]\w*)/, 23],
  [/^\s*palette\s+"([^"]*)"/, 18],
];

/** The outline: each declaration and handler, a block to its closing brace. */
function mockSymbols(lines: string[]) {
  const out: Array<{ name: string; kind: number; range: Range; selectionRange: Range }> = [];
  lines.forEach((text, line) => {
    for (const [re, kind] of SYMBOLS) {
      const m = re.exec(text);
      if (!m) continue;
      const name = m[1] ?? "";
      const indent = text.match(/^\s*/)?.[0].length ?? 0;
      let end = line;
      if (/[{[]\s*$/.test(text)) {
        for (let j = line + 1; j < lines.length; j++) {
          const l = lines[j] ?? "";
          if (/^\s*[}\]]/.test(l) && (l.match(/^\s*/)?.[0].length ?? 0) === indent) { end = j; break; }
        }
      }
      const at = Math.max(0, text.indexOf(name, indent));
      out.push({
        name: kind === 24 ? `on ${name}` : name,
        kind,
        range: span(line, 0, end, (lines[end] ?? "").length),
        selectionRange: span(line, at, line, at + name.length),
      });
      break;
    }
  });
  return out;
}

/** The call whose argument list holds the caret, and which argument. */
function callAt(before: string): { name: string; arg: number } | null {
  let depth = 0, commas = 0;
  for (let i = before.length - 1; i >= 0; i--) {
    const c = before[i];
    if (c === ")") depth++;
    else if (c === "(") {
      if (depth === 0) return { name: /([A-Za-z_]\w*)\s*$/.exec(before.slice(0, i))?.[1] ?? "", arg: commas };
      depth--;
    } else if (c === "," && depth === 0) commas++;
  }
  return null;
}

/** Each line indented four spaces a brace deep, trailing space dropped. */
function mockFormat(lines: string[]): string {
  let depth = 0;
  const out = lines.map((raw) => {
    const t = raw.replace(/\t/g, "    ").trim();
    const closes = /^[}\]]/.test(t);
    const l = t ? "    ".repeat(Math.max(0, depth - (closes ? 1 : 0))) + t : "";
    depth = Math.max(0, depth + (t.match(/[{[]/g)?.length ?? 0) - (t.match(/[}\]]/g)?.length ?? 0));
    return l;
  });
  return out.join("\n").replace(/\n*$/, "\n");
}

/** `POST /v1/rules/ide`'s answer, canned from the word lists. */
export function mockIde(source: string, op: IdeOp, line: number, character: number): unknown {
  const lines = source.split("\n");
  switch (op) {
    case "complete": {
      const text = lines[line] ?? "";
      if (/variant\.\w*$/.test(text.slice(0, character))) {
        return ["team_play", "score_limit", "time_limit", "lives", "respawn_time", "friendly_fire", "rounds", "engine"]
          .map((f) => ({ label: f, kind: 10, detail: "variant field" }));
      }
      const words = [...RESERVED, ...TYPES, ...SETS].map((k) => ({ label: k, kind: 14 }));
      const events = EVENTS.map((e) => ({
        label: eventLabelOf(e), kind: 23, detail: eventLabelOf(e), documentation: { kind: "markdown", value: e.words },
        insertText: eventSnippet(e), insertTextFormat: 2,
      }));
      const api = API_ENTRIES.map((e) => ({
        label: e.name, kind: 3, detail: apiSignature(e),
        documentation: { kind: "markdown", value: e.cap ? `needs \`requires ${e.cap}\`: ${CAP_WORDS[e.cap] ?? ""}` : "core" },
        insertText: apiSnippet(e), insertTextFormat: 2,
      }));
      // An indented line is a statement and gets calls; a top-level one gets handlers.
      return /^\s/.test(text) ? [...words, ...api] : [...words, ...events];
    }
    case "hover": {
      const w = wordAt(lines, line, character);
      const text = w ? hoverText(w.word) : null;
      return w && text ? { contents: { kind: "markdown", value: text }, range: span(line, w.start, line, w.end) } : null;
    }
    case "signature": {
      const call = callAt((lines[line] ?? "").slice(0, character));
      const e = call ? API_ENTRIES.find((x) => x.name === call.name) : undefined;
      if (!call || !e) return null;
      return {
        signatures: [{ label: apiSignature(e), documentation: { kind: "markdown", value: e.cap ? `needs requires ${e.cap}` : "core" }, parameters: e.args.map((label) => ({ label })) }],
        activeSignature: 0,
        activeParameter: call.arg,
      };
    }
    case "definition": {
      const w = wordAt(lines, line, character);
      const d = w ? declarationOf(lines, w.word) : null;
      return d ? [{ range: d }] : [];
    }
    case "references": {
      const w = wordAt(lines, line, character);
      if (!w) return [];
      const re = new RegExp(`\\b${w.word}\\b`, "g");
      return lines.flatMap((text, i) => [...text.matchAll(re)].map((m) => ({ range: span(i, m.index ?? 0, i, (m.index ?? 0) + w.word.length) })));
    }
    case "tokens": return mockTokens(lines);
    case "symbols": return mockSymbols(lines);
    case "format": {
      const bad = mockCheck(source).errors.find((e) => e.line > 0 && /[{}]/.test(e.message));
      return bad ? { error: `line ${bad.line}: ${bad.message}` } : { text: mockFormat(lines) };
    }
  }
}

interface Saved { hash: string; name: string; version: number; author: string; gamertag: string; saved_at: string; position: number; source: string }

const summary = (s: Saved) => ({ hash: s.hash, name: s.name, version: s.version, author: s.author, gamertag: s.gamertag, saved_at: s.saved_at, position: s.position });

export class MockRules {
  readonly saved = new Map<string, Saved>();

  /** `tools` false: a server from before the rules routes. */
  constructor(private world: World, private tools = true) {}

  async handle(req: Request, u: URL): Promise<Reply | null> {
    const path = u.pathname;
    if (!path.startsWith("/v1/rules")) return null;
    if (!this.tools) return fail(404, "not_found", "no such endpoint");
    const body = async (): Promise<Record<string, unknown> | null> => {
      try { const b = await req.json(); return b && typeof b === "object" ? b as Record<string, unknown> : null; } catch { return null; }
    };
    if (path === "/v1/rules/ide/legend" && req.method === "GET") return ok({ legend: MOCK_LEGEND });
    if (path === "/v1/rules/ide" && req.method === "POST") {
      const b = await body();
      if (typeof b?.source !== "string") return fail(400, "invalid", "body: missing field `source`");
      if (b.source.length > MAX_SOURCE) return fail(413, "too_large", `a script's source is at most ${MAX_SOURCE} bytes`);
      if (typeof b.op !== "string" || !IDE_OPS.has(b.op)) return fail(400, "invalid", "body: op is one of complete, hover, signature, definition, references, tokens, symbols, format");
      const line = typeof b.line === "number" ? Math.max(0, Math.floor(b.line)) : 0;
      const character = typeof b.character === "number" ? Math.max(0, Math.floor(b.character)) : 0;
      return ok(mockIde(b.source, b.op as IdeOp, line, character));
    }
    if (path === "/v1/rules/check" && req.method === "POST") {
      const b = await body();
      if (typeof b?.source !== "string") return fail(400, "invalid", "body: missing field `source`");
      if (b.source.length > MAX_SOURCE) return fail(413, "too_large", `a script's source is at most ${MAX_SOURCE} bytes`);
      return ok(mockCheck(b.source));
    }
    if (path === "/v1/rules/run" && req.method === "POST") {
      const b = await body();
      if (typeof b?.source !== "string") return fail(400, "invalid", "body: missing field `source`");
      const t = mockRun(b.source, (b.scenario ?? {}) as Record<string, unknown>);
      return t ? ok(t) : fail(400, "invalid", `scenario: a scenario runs at most ${MAX_TICKS} ticks`);
    }
    if (path === "/v1/rules/builtin" && req.method === "GET") return ok({ items: builtinScripts() });
    if (path === "/v1/rules" && req.method === "GET") {
      const author = u.searchParams.get("author")?.trim();
      if (!author) return fail(400, "invalid", "?author= (a gamertag or xuid) is required");
      const a = this.world.accounts.get(author) ?? this.world.accountByTag(author);
      if (!a) return fail(404, "not_found", "no such account");
      const items = [...this.saved.values()].filter((s) => s.author === a.xuid).reverse().map(summary);
      return ok({ items });
    }
    if (path === "/v1/rules" && req.method === "POST") {
      const claims = parseJws((req.headers.get("authorization") ?? "").replace(/^Bearer /, "").trim())?.payload as { iss?: string; lxm?: string } | undefined;
      const caller = /^acct:([0-9a-f]{16})@/.exec(claims?.iss ?? "")?.[1];
      if (!caller || claims?.lxm !== "content.write") return fail(401, "unauthorized", "this needs a content.write token");
      const b = await body();
      if (typeof b?.source !== "string") return fail(400, "invalid", "body: missing field `source`");
      const c = mockCheck(b.source);
      if (!c.ok || !c.hash) return fail(400, "invalid", `the script does not verify: ${c.errors[0]?.message ?? "it does not verify"}`);
      const had = this.saved.get(c.hash);
      if (had) return ok({ ok: true, saved: false, script: had });
      const s: Saved = {
        hash: c.hash, name: c.name ?? "", version: c.version ?? 0, author: caller,
        gamertag: this.world.accounts.get(caller)?.gamertag ?? "", saved_at: new Date().toISOString(),
        position: this.world.log.length, source: b.source,
      };
      this.saved.set(c.hash, s);
      return ok({ ok: true, saved: true, script: s }, 201);
    }
    const m = path.match(/^\/v1\/rules\/([0-9a-f]{64})(\/ir)?$/);
    if (m && req.method === "GET") {
      const s = this.saved.get(m[1]!);
      if (!s) return fail(404, "not_found", "no such script");
      if (m[2]) return fail(501, "unimplemented", "the mock has no IR");
      return ok(s);
    }
    return fail(404, "not_found", "no such script");
  }
}
