// The game type script workbench's pure half (docs/game/rules-engine.md,
// "Workbench"; docs/live/live-api.md, "Rules scripts"): the server's check
// and trace answers read defensively, the generated matches and their
// summary, the scenario built into the run request, times as they are
// typed and shown, the trace as lines and as a
// timeline with the final standings, errors mapped onto the editor's text,
// failures put in words, why a button is off, the editor's tab key, and
// the drafts kept in localStorage. The server compiles and runs; nothing
// here knows the language.

import { ApiError } from "../api/http";
import type { LocalAccount } from "../state/account";
import { issueToken } from "../admin/token";

/** Saving takes a `content.write` token, as other publishing does. */
export const CONTENT_WRITE = "content.write";
/** The server's caps (rules::workbench, live_proto::rules). */
export const MAX_SOURCE = 256 * 1024;
export const MAX_TICKS = 36_000;
export const TICKS_PER_SECOND = 30;
/** Spaces a tab inserts, as the built-ins are indented. */
export const INDENT = "    ";

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const optStr = (v: unknown): string | null => (typeof v === "string" ? v : null);
const strs = (v: unknown): string[] => arr(v).filter((x): x is string => typeof x === "string");

// ---------- check ----------

/** An error at a line and column (from 1); 0 for the whole script. */
export interface Problem { line: number; col: number; message: string }
export interface HandlerCost { event: string; worst_ops: number }
export interface SlotUse { scope: string; used: number; limit: number }
export interface CapLine { name: string; words: string; grantable: boolean; known: boolean }
export interface CheckReport {
  worst_tick_ops: number;
  budget: number;
  fits: boolean;
  costliest: string | null;
  handlers: HandlerCost[];
  slots: SlotUse[];
  ir_bytes: number;
  ir_limit: number;
  capabilities: CapLine[];
  unhostable: string[];
}
/** An option a script declares (`option kill_points: int 1..5 = 1 ...`),
 *  as the check lists it when the server sends them. */
export interface ScriptOption {
  name: string;
  label: string;
  description: string | null;
  kind: "int" | "bool" | "enum";
  lo: number;
  hi: number;
  default: number | boolean | string;
  /** An enum's value names. */
  values: string[];
}
/** The retail game type a script rides on (its `carrier` line). */
export interface Carrier { name: string; engine: number; label: string }
export interface CheckResult {
  ok: boolean;
  hash: string | null;
  name: string | null;
  version: number | null;
  errors: Problem[];
  report: CheckReport | null;
  /** The declared options (the check sends them when it is ok and the
   *  script has some); empty otherwise. */
  options: ScriptOption[];
  /** The carrier, when the server's check names it. */
  carrier: Carrier | null;
}

/** The carriers, by their `carrier` keyword, with the retail engine index
 *  (rules::caps). */
export const CARRIERS: ReadonlyArray<Carrier> = [
  { name: "ctf", engine: 1, label: "Capture the Flag" },
  { name: "slayer", engine: 2, label: "Slayer" },
  { name: "oddball", engine: 3, label: "Oddball" },
  { name: "king", engine: 4, label: "King of the Hill" },
  { name: "juggernaut", engine: 7, label: "Juggernaut" },
  { name: "territories", engine: 8, label: "Territories" },
  { name: "assault", engine: 9, label: "Assault" },
];

/** A carrier from its keyword or engine index. */
export function carrierFor(v: string | number): Carrier | null {
  return CARRIERS.find((c) => (typeof v === "number" ? c.engine === v : c.name === v.trim().toLowerCase())) ?? null;
}

/** The carrier a script's header names, read off its `carrier` line. */
export function carrierOf(source: string): Carrier | null {
  const m = /^\s*carrier\s+([a-z_]+)/m.exec(source);
  return m ? carrierFor(m[1]!) : null;
}

function parseCarrier(v: unknown): Carrier | null {
  if (typeof v === "string" || typeof v === "number") return carrierFor(v);
  const c = obj(v);
  if (typeof c.engine === "number") return carrierFor(c.engine);
  if (typeof c.name === "string") return carrierFor(c.name);
  return null;
}

/** The options of a check, read defensively: an unknown kind is dropped. */
export function parseOptions(v: unknown): ScriptOption[] {
  const out: ScriptOption[] = [];
  for (const o of arr(v).map(obj)) {
    const name = str(o.name), kind = o.kind;
    if (!name || (kind !== "int" && kind !== "bool" && kind !== "enum")) continue;
    const values = strs(o.values);
    const lo = num(o.lo), hi = typeof o.hi === "number" ? o.hi : kind === "enum" ? Math.max(0, values.length - 1) : lo;
    const dflt = kind === "bool" ? o.default === true
      : kind === "enum" ? (typeof o.default === "string" ? o.default : values[num(o.default)] ?? values[0] ?? "")
      : typeof o.default === "number" ? o.default : lo;
    out.push({ name, label: str(o.label) || name, description: optStr(o.description), kind, lo, hi, default: dflt, values });
  }
  return out;
}

export function parseProblems(v: unknown): Problem[] {
  return arr(v).map(obj).map((p) => ({ line: Math.max(0, Math.floor(num(p.line))), col: Math.max(0, Math.floor(num(p.col))), message: str(p.message) || "error" }));
}

function parseReport(v: unknown): CheckReport | null {
  if (!v || typeof v !== "object") return null;
  const r = obj(v);
  return {
    worst_tick_ops: num(r.worst_tick_ops),
    budget: num(r.budget) || 20_000,
    fits: r.fits === true,
    costliest: optStr(r.costliest),
    handlers: arr(r.handlers).map(obj).map((h) => ({ event: str(h.event), worst_ops: num(h.worst_ops) })),
    slots: arr(r.slots).map(obj).map((s) => ({ scope: str(s.scope), used: num(s.used), limit: num(s.limit) })),
    ir_bytes: num(r.ir_bytes),
    ir_limit: num(r.ir_limit) || 64 * 1024,
    capabilities: arr(r.capabilities).map(obj).map((c) => ({ name: str(c.name), words: str(c.words), grantable: c.grantable === true, known: c.known !== false })),
    unhostable: strs(r.unhostable),
  };
}

/** `POST /v1/rules/check`'s answer. */
export function parseCheck(body: unknown): CheckResult {
  const b = obj(body);
  return {
    ok: b.ok === true,
    hash: optStr(b.hash),
    name: optStr(b.name),
    version: typeof b.version === "number" ? b.version : null,
    errors: parseProblems(b.errors),
    report: parseReport(b.report),
    options: parseOptions(b.options ?? obj(b.report).options),
    carrier: parseCarrier(b.carrier ?? obj(b.report).carrier),
  };
}

/** Where an error points, as the list shows it. */
export const problemPlace = (p: Problem): string => (p.line > 0 ? `line ${p.line}, col ${p.col}` : "script");

/** How full a meter is: its share (0..1, capped) and its tone. */
export function meter(used: number, limit: number): { share: number; tone: "ok" | "warn" | "over" } {
  if (limit <= 0) return { share: 0, tone: "ok" };
  const share = used / limit;
  return { share: Math.min(1, Math.max(0, share)), tone: share > 1 ? "over" : share >= 0.75 ? "warn" : "ok" };
}

/** "1,234 / 20,000 ops". */
export const meterText = (used: number, limit: number, unit: string): string =>
  `${used.toLocaleString("en-US")} / ${limit.toLocaleString("en-US")} ${unit}`;

// ---------- the editor ----------

/** The offset of each line's first character. */
export function lineStarts(src: string): number[] {
  const out = [0];
  for (let i = 0; i < src.length; i++) if (src[i] === "\n") out.push(i + 1);
  return out;
}

/** The gutter's numbers, one a line. */
export const gutter = (src: string): string => lineStarts(src).map((_, i) => String(i + 1)).join("\n");

/** The text offset of a problem's line and column, clamped to the text and
 *  to its line; 0 for a whole-script problem. */
export function offsetOf(src: string, line: number, col: number): number {
  if (line <= 0) return 0;
  const starts = lineStarts(src);
  const at = Math.min(line, starts.length) - 1;
  const start = starts[at] ?? 0;
  const end = at + 1 < starts.length ? (starts[at + 1] ?? src.length) - 1 : src.length;
  return Math.min(end, start + Math.max(0, col - 1));
}

/** The line (from 1) that holds `offset`. */
export function lineAt(src: string, offset: number): number {
  let line = 1;
  for (let i = 0; i < Math.min(offset, src.length); i++) if (src[i] === "\n") line++;
  return line;
}

export interface Edit { value: string; start: number; end: number }

/** The editor's Tab: spaces at the caret, or a whole selection of lines
 *  indented; Shift+Tab takes up to one indent off each selected line. */
export function tabKey(value: string, start: number, end: number, shift: boolean): Edit {
  if (!shift && start === end) {
    return { value: value.slice(0, start) + INDENT + value.slice(end), start: start + INDENT.length, end: start + INDENT.length };
  }
  const first = value.lastIndexOf("\n", start - 1) + 1;
  const tail = end > start && value[end - 1] === "\n" ? end - 1 : end;
  const lineEnd = value.indexOf("\n", tail);
  const last = lineEnd === -1 ? value.length : lineEnd;
  const lines = value.slice(first, last).split("\n");
  let startShift = 0, total = 0;
  const changed = lines.map((l, i) => {
    if (!shift) {
      if (i === 0) startShift = INDENT.length;
      total += INDENT.length;
      return INDENT + l;
    }
    const cut = Math.min(INDENT.length, l.match(/^ */)?.[0].length ?? 0);
    if (i === 0) startShift = -Math.min(cut, start - first);
    total -= cut;
    return l.slice(cut);
  });
  const next = value.slice(0, first) + changed.join("\n") + value.slice(last);
  return { value: next, start: Math.max(first, start + startShift), end: Math.max(first, end + total) };
}

// ---------- scenarios ----------

export type EventType =
  | "kill" | "death" | "spawn" | "join" | "quit" | "press" | "pickup" | "drop"
  | "enter" | "leave" | "bounce" | "edit" | "squad_dead" | "actor_kill" | "actor_assist"
  | "round_start" | "round_end";
export type EventField = "player" | "victim" | "killer" | "assist" | "object" | "zone" | "squad" | "class";

/** The events a scenario offers, with the fields each one takes. */
export const EVENT_TYPES: ReadonlyArray<{ type: EventType; label: string; fields: EventField[] }> = [
  { type: "kill", label: "Kill", fields: ["victim", "killer"] },
  { type: "death", label: "Death", fields: ["player"] },
  { type: "spawn", label: "Spawn", fields: ["player"] },
  { type: "join", label: "Join", fields: ["player"] },
  { type: "quit", label: "Quit", fields: ["player"] },
  { type: "press", label: "Press", fields: ["player", "object"] },
  { type: "pickup", label: "Pick up", fields: ["player", "object"] },
  { type: "drop", label: "Drop", fields: ["player", "object"] },
  { type: "enter", label: "Zone enter", fields: ["zone", "object"] },
  { type: "leave", label: "Zone leave", fields: ["zone", "object"] },
  { type: "bounce", label: "Bounce", fields: ["player", "object"] },
  { type: "edit", label: "Edit", fields: ["player", "object"] },
  { type: "squad_dead", label: "Squad dead", fields: ["squad"] },
  { type: "actor_kill", label: "AI kill", fields: ["killer", "class"] },
  { type: "actor_assist", label: "AI assist", fields: ["killer", "assist", "class"] },
  { type: "round_start", label: "Round start", fields: [] },
  { type: "round_end", label: "Round end", fields: [] },
];

export const eventLabel = (t: EventType): string => EVENT_TYPES.find((e) => e.type === t)?.label ?? t;
export const eventFields = (t: EventType): EventField[] => EVENT_TYPES.find((e) => e.type === t)?.fields ?? [];

/** A seated player; `unit` is the object the player drives, which an
 *  `enter` or `leave` names (a generated match seats one each). */
export interface ScenarioPlayer { slot: number; team: number; lives: number; unit?: number }
/** A labelled object on the fake map: what `objects labelled "x"` visits. */
export interface ScenarioObject { id: number; label: string; number: number; team: number | null }
/** One event of the tick list. `killer` null is a death by the world. */
export interface ScenarioEvent {
  tick: number;
  type: EventType;
  player?: number;
  victim?: number;
  killer?: number | null;
  assist?: number | null;
  object?: number;
  zone?: number;
  squad?: number;
  class?: string;
}
export interface ScenarioDraft {
  players: ScenarioPlayer[];
  events: ScenarioEvent[];
  /** The record's fields a generated match was played under (the score
   *  and time limits and the carrier's settings). `teamPlay` wins over
   *  its `team_play`. */
  variant?: Record<string, number>;
  /** The labelled objects a generated match placed. */
  objects?: ScenarioObject[];
  /** The last tick run. */
  ticks: number;
  /** `round_time_left()` at the start, in ticks. */
  roundTime: number;
  seed: number;
  teamPlay: boolean;
}

export function defaultScenario(): ScenarioDraft {
  return {
    players: [{ slot: 0, team: 0, lives: 0 }, { slot: 1, team: 1, lives: 0 }],
    events: [
      { tick: 30, type: "kill", victim: 1, killer: 0 },
      { tick: 60, type: "spawn", player: 1 },
      { tick: 90, type: "kill", victim: 0, killer: null },
    ],
    ticks: 300,
    roundTime: 600 * TICKS_PER_SECOND,
    seed: 1,
    teamPlay: false,
  };
}

/** A new event for the tick list, filled from the seated players. */
export function newEvent(type: EventType, tick: number, players: ScenarioPlayer[]): ScenarioEvent {
  const a = players[0]?.slot ?? 0, b = players[1]?.slot ?? a;
  switch (type) {
    case "kill": return { tick, type, victim: b, killer: a };
    case "enter": case "leave": return { tick, type, zone: 0, object: 0 };
    case "press": case "pickup": case "drop": case "bounce": case "edit": return { tick, type, player: a, object: 0 };
    case "squad_dead": return { tick, type, squad: 0 };
    case "actor_kill": return { tick, type, killer: a, class: "grunt" };
    case "actor_assist": return { tick, type, killer: a, assist: b, class: "grunt" };
    case "round_start": case "round_end": return { tick, type };
    default: return { tick, type, player: a };
  }
}

/** The next free player slot, or null when all 16 are seated. */
export function nextSlot(players: ScenarioPlayer[]): number | null {
  for (let s = 0; s < 16; s++) if (!players.some((p) => p.slot === s)) return s;
  return null;
}

/** The events in tick order, events of one tick in their listed order. */
export function sortEvents(events: ScenarioEvent[]): ScenarioEvent[] {
  return events.map((e, i) => ({ e, i })).sort((a, b) => a.e.tick - b.e.tick || a.i - b.i).map((x) => x.e);
}

/** Where the next event goes: a second after the last one. */
export function nextEventTick(d: ScenarioDraft): number {
  const last = d.events.reduce((m, e) => Math.max(m, e.tick), 0);
  return Math.min(MAX_TICKS, last + TICKS_PER_SECOND);
}

/** The run long enough to reach `tick` with a second to spare. */
const reaching = (d: ScenarioDraft, tick: number): number => (tick > d.ticks ? Math.min(MAX_TICKS, tick + TICKS_PER_SECOND) : d.ticks);

/** A scenario with one more event, the list in tick order and the run
 *  lengthened to reach it. */
export function withEvent(d: ScenarioDraft, e: ScenarioEvent): ScenarioDraft {
  const tick = Math.min(MAX_TICKS, Math.max(0, e.tick));
  return { ...d, events: sortEvents([...d.events, { ...e, tick }]), ticks: reaching(d, tick) };
}

/** A scenario with event `i` moved to `tick`, likewise. */
export function moveEvent(d: ScenarioDraft, i: number, tick: number): ScenarioDraft {
  const at = Math.min(MAX_TICKS, Math.max(0, tick));
  return { ...d, events: sortEvents(d.events.map((e, j) => (j === i ? { ...e, tick: at } : e))), ticks: reaching(d, at) };
}

/** A player as the page names one. */
export const playerName = (slot: number): string => `Player ${slot}`;

/** A player's own object is this plus the slot, and the objectives a
 *  script spawns count up from the other (rules::workbench). */
export const UNIT_BASE = 0x1000;
export const SPAWNED_BASE = 0x4000;

/** An object as the page names one: a player by its own object, a
 *  labelled object by its label and number, a spawned objective by its
 *  order, else its number. */
export function objectName(id: number, objects: ScenarioObject[] = []): string {
  if (id >= UNIT_BASE && id < UNIT_BASE + 16) return playerName(id - UNIT_BASE);
  const o = objects.find((x) => x.id === id);
  if (o) return o.team === null ? `${o.label} ${o.number}` : `${o.label} (team ${o.team})`;
  if (id >= SPAWNED_BASE && id < SPAWNED_BASE + 256) return `objective ${id - SPAWNED_BASE + 1}`;
  return `object ${id}`;
}

const capital = (s: string): string => (s ? s[0]!.toUpperCase() + s.slice(1) : s);

/** An event as a sentence: "Player 0 killed Player 1". */
export function describeEvent(e: ScenarioEvent, objects: ScenarioObject[] = []): string {
  const who = playerName(e.player ?? 0), thing = objectName(e.object ?? 0, objects);
  const by = typeof e.killer === "number" ? playerName(e.killer) : "Nobody";
  switch (e.type) {
    case "kill": {
      const victim = playerName(e.victim ?? 0);
      if (typeof e.killer !== "number") return `The world killed ${victim}`;
      return e.killer === e.victim ? `${victim} killed themselves` : `${playerName(e.killer)} killed ${victim}`;
    }
    case "death": return `${who} died`;
    case "spawn": return `${who} spawned`;
    case "join": return `${who} joined`;
    case "quit": return `${who} quit`;
    case "press": return `${who} pressed ${thing}`;
    case "pickup": return `${who} picked up ${thing}`;
    case "drop": return `${who} dropped ${thing}`;
    case "enter": return `${capital(thing)} entered ${objects.length ? objectName(e.zone ?? 0, objects) : `zone ${e.zone ?? 0}`}`;
    case "leave": return `${capital(thing)} left ${objects.length ? objectName(e.zone ?? 0, objects) : `zone ${e.zone ?? 0}`}`;
    case "bounce": return `${who} bounced on ${thing}`;
    case "edit": return `${who} edited ${thing}`;
    case "squad_dead": return `Squad ${e.squad ?? 0} died`;
    case "actor_kill": return `${by} killed a ${e.class || "creature"}`;
    case "actor_assist": return `${typeof e.assist === "number" ? playerName(e.assist) : "Nobody"} helped ${by.replace(/^Nobody$/, "nobody")} kill a ${e.class || "creature"}`;
    case "round_start": return "The round started";
    case "round_end": return "The round ended";
  }
}

/** What is wrong with a scenario before it is sent, in words. */
export function scenarioProblems(d: ScenarioDraft): string[] {
  const out: string[] = [];
  if (!Number.isInteger(d.ticks) || d.ticks < 0 || d.ticks > MAX_TICKS) out.push(`run between 0 and ${MAX_TICKS} ticks`);
  const seated = new Set(d.players.map((p) => p.slot));
  if (seated.size !== d.players.length) out.push("two players share a slot");
  for (const p of d.players) {
    if (p.slot < 0 || p.slot > 15) out.push(`slot ${p.slot} is out of range (0 to 15)`);
    if (p.team < 0 || p.team > 7) out.push(`team ${p.team} is out of range (0 to 7)`);
  }
  const joined = new Set<number>();
  for (const e of [...d.events].sort((a, b) => a.tick - b.tick)) {
    if (e.tick > d.ticks) out.push(`a ${eventLabel(e.type)} at tick ${e.tick} is past the last tick (${d.ticks})`);
    if (e.tick < 0) out.push(`a ${eventLabel(e.type)} has a negative tick`);
    if (e.type === "join" && e.player !== undefined) joined.add(e.player);
    for (const f of ["player", "victim", "killer", "assist"] as const) {
      const s = e[f];
      if (typeof s === "number" && e.type !== "join" && !seated.has(s) && !joined.has(s)) {
        out.push(`a ${eventLabel(e.type)} at tick ${e.tick} names player ${s}, who is not seated`);
      }
    }
  }
  return out;
}

/** The wire form of one event. */
export function wireEvent(e: ScenarioEvent): Record<string, unknown> {
  const out: Record<string, unknown> = { type: e.type };
  for (const f of eventFields(e.type)) {
    const v = e[f];
    if (f === "killer" || f === "assist") out[f] = typeof v === "number" ? v : null;
    else if (f === "class") out.class = typeof v === "string" ? v : "";
    else out[f] = typeof v === "number" ? v : 0;
  }
  return out;
}

/** `POST /v1/rules/run`'s scenario: the tick list grouped by tick, in tick
 *  order, events of one tick in the order they were listed. */
export function buildScenario(d: ScenarioDraft): Record<string, unknown> {
  const byTick = new Map<number, Record<string, unknown>[]>();
  const order = d.events.map((e, i) => ({ e, i })).sort((a, b) => a.e.tick - b.e.tick || a.i - b.i);
  for (const { e } of order) {
    const list = byTick.get(e.tick) ?? [];
    list.push(wireEvent(e));
    byTick.set(e.tick, list);
  }
  return {
    players: d.players.map(wireSeat),
    ...(d.objects?.length ? { labelled: d.objects.map((o) => ({ label: o.label, object: o.id, number: o.number, ...(o.team === null ? {} : { team: o.team }) })) } : {}),
    variant: { ...d.variant, team_play: d.teamPlay ? 1 : 0 },
    seed: d.seed,
    round_time: d.roundTime,
    ticks: d.ticks,
    start: true,
    timeline: [...byTick.entries()].map(([tick, events]) => ({ tick, events })),
  };
}

const whole = (x: unknown, dflt: number): number => (typeof x === "number" && Number.isInteger(x) ? x : dflt);

const wireSeat = (p: ScenarioPlayer): Record<string, unknown> => ({ slot: p.slot, team: p.team, lives: p.lives, ...(typeof p.unit === "number" ? { unit: p.unit } : {}) });
const readSeat = (v: unknown): ScenarioPlayer => {
  const p = obj(v);
  return { slot: whole(p.slot, 0), team: whole(p.team, 0), lives: whole(p.lives, 0), ...(typeof p.unit === "number" && Number.isInteger(p.unit) ? { unit: p.unit } : {}) };
};

/** One event read from a draft or from the wire; null for a type the page
 *  does not know. */
function readEvent(v: unknown, tick: number): ScenarioEvent | null {
  const e = obj(v);
  if (!EVENT_TYPES.some((t) => t.type === e.type)) return null;
  const out: ScenarioEvent = { tick, type: e.type as EventType };
  for (const f of ["player", "victim", "object", "zone", "squad"] as const) if (typeof e[f] === "number") out[f] = e[f] as number;
  if (e.type === "kill" || e.type === "actor_kill" || e.type === "actor_assist") out.killer = typeof e.killer === "number" ? e.killer : null;
  if (e.type === "actor_assist") out.assist = typeof e.assist === "number" ? e.assist : null;
  if (typeof e.class === "string") out.class = e.class;
  return out;
}

const readVariant = (v: unknown): Record<string, number> | undefined => {
  const out = Object.entries(obj(v)).filter((kv): kv is [string, number] => typeof kv[1] === "number" && Number.isInteger(kv[1]));
  return out.length ? Object.fromEntries(out) : undefined;
};
const readObjects = (v: unknown): ScenarioObject[] | undefined => {
  // The wire names the object `object` (`labelled`); a kept draft, `id`.
  const out = arr(v).map(obj).map((o) => ({ ...o, id: typeof o.object === "number" ? o.object : o.id }))
    .filter((o) => typeof o.id === "number" && typeof o.label === "string")
    .map((o) => ({ id: o.id as number, label: o.label as string, number: whole(o.number, 0), team: typeof o.team === "number" ? o.team : null }));
  return out.length ? out : undefined;
};

/** A scenario as the server sends one (a generated match's, in
 *  `POST /v1/rules/run`'s shape) as a draft the page shows and edits. */
export function draftOfWire(v: unknown): ScenarioDraft {
  const b = obj(v), variant = readVariant(b.variant), objects = readObjects(b.labelled);
  const events = arr(b.timeline).map(obj).flatMap((t) => arr(t.events).map((e) => readEvent(e, whole(t.tick, 0)))).filter((e): e is ScenarioEvent => e !== null);
  const last = events.reduce((m, e) => Math.max(m, e.tick), 0);
  return {
    players: arr(b.players).map(readSeat),
    events: sortEvents(events),
    ...(variant ? { variant } : {}),
    ...(objects ? { objects } : {}),
    ticks: whole(b.ticks, last),
    roundTime: whole(b.round_time, 0),
    seed: whole(b.seed, 0),
    teamPlay: variant?.team_play === 1,
  };
}

/** A scenario read back from a draft, any part it can't read replaced by
 *  the default's. */
export function parseScenario(v: unknown): ScenarioDraft {
  const d = defaultScenario(), b = obj(v);
  const int = whole;
  const variant = readVariant(b.variant), objects = readObjects(b.objects);
  return {
    players: Array.isArray(b.players) ? b.players.map(readSeat) : d.players,
    events: Array.isArray(b.events)
      ? b.events.map((e) => readEvent(e, int(obj(e).tick, 0))).filter((e): e is ScenarioEvent => e !== null)
      : d.events,
    ...(variant ? { variant } : {}),
    ...(objects ? { objects } : {}),
    ticks: int(b.ticks, d.ticks),
    roundTime: int(b.roundTime, d.roundTime),
    seed: int(b.seed, d.seed),
    teamPlay: b.teamPlay === true,
  };
}

// ---------- traces ----------

export interface TraceCall { entry: string; args: string[] }
export interface TraceVar { name: string; value: string }
export interface TracePlayer { slot: number; team: number; alive: boolean; score: number; lives: number; vars: TraceVar[] }
export interface TraceTick {
  tick: number;
  ran: string[];
  calls: TraceCall[];
  ops: number;
  warnings: string[];
  globals: TraceVar[];
  players: TracePlayer[];
  error: string | null;
}
export interface Trace {
  ok: boolean;
  hash: string;
  ticks: TraceTick[];
  last_tick: number;
  error: string | null;
  round_ended: number | null;
  /** The record's limit the run stopped on, at `last_tick`. */
  limit?: "score" | "time" | null;
  cut: string | null;
  ops: number;
  /** The most ops one tick spent. */
  worst_tick_ops?: number;
  scores: Array<{ slot: number; score: number }>;
}
/** A run whose script did not build. */
export interface RunFailed { ok: false; errors: Problem[] }

const vars = (v: unknown): TraceVar[] => arr(v).map(obj).map((x) => ({ name: str(x.name), value: str(x.value) }));

/** `POST /v1/rules/run`'s answer. */
export function parseTrace(body: unknown): Trace | RunFailed {
  const b = obj(body);
  if (b.ok !== true && Array.isArray(b.errors)) return { ok: false, errors: parseProblems(b.errors) };
  return {
    ok: b.ok === true,
    hash: str(b.hash),
    ticks: arr(b.ticks).map(obj).map((t) => ({
      tick: num(t.tick),
      ran: strs(t.ran),
      calls: arr(t.calls).map(obj).map((c) => ({ entry: str(c.entry), args: strs(c.args) })),
      ops: num(t.ops),
      warnings: strs(t.warnings),
      globals: vars(t.globals),
      players: arr(t.players).map(obj).map((p) => ({ slot: num(p.slot), team: num(p.team), alive: p.alive === true, score: num(p.score), lives: num(p.lives), vars: vars(p.vars) })),
      error: optStr(t.error),
    })),
    last_tick: num(b.last_tick),
    error: optStr(b.error),
    round_ended: typeof b.round_ended === "number" ? b.round_ended : null,
    limit: b.limit === "score" || b.limit === "time" ? b.limit : null,
    cut: optStr(b.cut),
    ops: num(b.ops),
    worst_tick_ops: num(b.worst_tick_ops),
    scores: arr(b.scores).map(obj).map((s) => ({ slot: num(s.slot), score: num(s.score) })),
  };
}

export const runFailed = (t: Trace | RunFailed): t is RunFailed => "errors" in t;

/** A tick as match time: tick 75 is "0:02.50". */
export function tickTime(tick: number): string {
  const total = Math.max(0, tick) / TICKS_PER_SECOND;
  const m = Math.floor(total / 60), s = total - m * 60;
  return `${m}:${s.toFixed(2).padStart(5, "0")}`;
}

/** A time as it is typed, in ticks: "0:02.50", seconds ("2.5", "2.5s")
 *  or ticks ("75t"); null when it reads as none of them. */
export function parseTickTime(text: string): number | null {
  const s = text.trim().toLowerCase();
  let m = /^(\d+)\s*t(?:icks?)?$/.exec(s);
  if (m) return Number(m[1]);
  m = /^(\d+):(\d{1,2}(?:\.\d*)?)$/.exec(s);
  if (m) {
    const sec = Number(m[2]);
    return sec < 60 ? Math.round((Number(m[1]) * 60 + sec) * TICKS_PER_SECOND) : null;
  }
  m = /^(\d+\.?\d*|\.\d+)\s*s?$/.exec(s);
  return m ? Math.round(Number(m[1]) * TICKS_PER_SECOND) : null;
}

/** A call as the script would write it: `score(player 0, 1)`. */
export const formatCall = (c: TraceCall): string => `${c.entry}(${c.args.join(", ")})`;

/** A variable list: "infected = true, wave = 2". */
export const formatVars = (vs: TraceVar[]): string => vs.map((v) => `${v.name} = ${v.value}`).join(", ");

/** The trace as plain lines, one a tick, for the text view and copying. */
export function traceLines(t: Trace): string[] {
  const out = t.ticks.map((k) => {
    const parts = [`tick ${k.tick} (${tickTime(k.tick)})`];
    if (k.ran.length) parts.push(`ran ${k.ran.map((r) => `on ${r}`).join(", ")}`);
    if (k.calls.length) parts.push(k.calls.map(formatCall).join("; "));
    if (k.globals.length) parts.push(formatVars(k.globals));
    for (const w of k.warnings) parts.push(`warning: ${w}`);
    if (k.error) parts.push(k.error);
    return parts.join(" | ");
  });
  if (t.round_ended !== null) out.push(`the round ended at tick ${t.round_ended}`);
  else if (t.limit) out.push(`the game ended at tick ${t.last_tick} on the ${t.limit} limit`);
  else out.push(`ran to tick ${t.last_tick}`);
  if (t.cut) out.push(t.cut);
  return out;
}

// ---------- results ----------

export interface ScoreChange { slot: number; delta: number; score: number }
/** One moment of a run: what the scenario did, what the script did about
 *  it, and what changed. */
export interface TimelineRow {
  tick: number;
  /** The scenario's events at this tick, as sentences. */
  happened: string[];
  /** The handlers that ran. */
  ran: string[];
  /** The calls they made, as the script writes them. */
  calls: string[];
  scored: ScoreChange[];
  /** Other changes: a team, lives, a variable. */
  changes: string[];
  warnings: string[];
  error: string | null;
}

/** A run as a timeline: every tick the scenario put an event in or the
 *  trace kept, up to where the run stopped. A score, team, lives count or
 *  variable is listed in the tick it changed. */
export function timeline(t: Trace, d: ScenarioDraft, teamLabel: (team: number) => string = (n) => `team ${n}`): TimelineRow[] {
  const end = t.round_ended ?? t.last_tick;
  const traced = new Map(t.ticks.map((k) => [k.tick, k]));
  const events = sortEvents(d.events).filter((e) => e.tick <= end);
  const ticks = [...new Set([...events.map((e) => e.tick), ...traced.keys()])].sort((a, b) => a - b);
  const seen = new Map(d.players.map((p) => [p.slot, { score: 0, team: p.team, lives: p.lives }]));
  const values = new Map<string, string>();
  const changed = (key: string, value: string): boolean => {
    if (values.get(key) === value) return false;
    values.set(key, value);
    return true;
  };
  return ticks.map((tick) => {
    const k = traced.get(tick);
    const row: TimelineRow = {
      tick,
      happened: events.filter((e) => e.tick === tick).map((e) => describeEvent(e, d.objects)),
      ran: k?.ran ?? [],
      calls: (k?.calls ?? []).map(formatCall),
      scored: [],
      changes: [],
      warnings: k?.warnings ?? [],
      error: k?.error ?? null,
    };
    for (const g of k?.globals ?? []) if (changed(`g:${g.name}`, g.value)) row.changes.push(`${g.name} = ${g.value}`);
    for (const p of k?.players ?? []) {
      const was = seen.get(p.slot);
      const name = playerName(p.slot);
      if (p.score !== (was?.score ?? 0)) row.scored.push({ slot: p.slot, delta: p.score - (was?.score ?? 0), score: p.score });
      if (was && p.team !== was.team) row.changes.push(`${name} moved to ${teamLabel(p.team)}`);
      if (was && p.lives !== was.lives) row.changes.push(`${name} has ${p.lives} ${p.lives === 1 ? "life" : "lives"}`);
      for (const v of p.vars) if (changed(`p${p.slot}:${v.name}`, v.value)) row.changes.push(`${name}: ${v.name} = ${v.value}`);
      seen.set(p.slot, { score: p.score, team: p.team, lives: p.lives });
    }
    return row;
  });
}

/** "+1", "-2". */
export const signed = (n: number): string => (n > 0 ? `+${n}` : String(n));

export interface Standing { place: number; slot: number; team: number; score: number }

/** The final scores, best first; equal scores share a place. A player's
 *  team is the last the trace gave, else the scenario's. */
export function standings(t: Trace, d: ScenarioDraft): Standing[] {
  const team = new Map(d.players.map((p) => [p.slot, p.team]));
  for (const k of t.ticks) for (const p of k.players) team.set(p.slot, p.team);
  const rows = [...t.scores].sort((a, b) => b.score - a.score || a.slot - b.slot);
  return rows.map((s) => ({ place: 1 + rows.filter((o) => o.score > s.score).length, slot: s.slot, team: team.get(s.slot) ?? 0, score: s.score }));
}

/** The standings summed by team, best first. */
export function teamStandings(rows: Standing[]): Array<{ place: number; team: number; score: number }> {
  const sums = new Map<number, number>();
  for (const r of rows) sums.set(r.team, (sums.get(r.team) ?? 0) + r.score);
  const out = [...sums.entries()].map(([team, score]) => ({ team, score })).sort((a, b) => b.score - a.score || a.team - b.team);
  return out.map((s) => ({ place: 1 + out.filter((o) => o.score > s.score).length, ...s }));
}

// ---------- generated matches ----------

/** What the page asks the generator for. */
export interface SimSetup { players: number; teamPlay: boolean; seed: number }
/** Matches a check is followed by. */
export const SIM_MATCHES = 25;
export const SIM_PLAYERS = { min: 2, max: 16 } as const;
export const MAX_SEED = 999_999;
export const defaultSetup = (): SimSetup => ({ players: 8, teamPlay: false, seed: 1 });

const clamp = (n: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, n));

/** A setup with every part in range, whatever was typed or kept. */
export function parseSetup(v: unknown): SimSetup {
  const b = obj(v), d = defaultSetup();
  return {
    players: clamp(whole(b.players, d.players), SIM_PLAYERS.min, SIM_PLAYERS.max),
    teamPlay: b.teamPlay === true,
    seed: clamp(whole(b.seed, d.seed), 0, MAX_SEED),
  };
}

/** A new seed from the die: never the one it had. */
export function rerolled(seed: number, random: () => number = Math.random): number {
  const next = 1 + Math.floor(random() * MAX_SEED);
  return next === seed ? (seed % MAX_SEED) + 1 : next;
}

/** `POST /v1/rules/simulate`'s body: `matches` seeds from `seed`. */
export function simRequest(source: string, setup: SimSetup, matches: number, seed: number = setup.seed): Record<string, unknown> {
  return { source, players: setup.players, teams: setup.teamPlay ? 2 : 0, seed, matches };
}

export type SimEnd = "script" | "score_limit" | "time_limit" | "error" | "cap";
export interface SimWinner { kind: "player" | "team" | "draw" | "nobody"; index: number | null }
export interface SimRow { seed: number; end: SimEnd; winner: SimWinner; ticks: number; top_score: number; ops: number; worst_tick_ops: number }
export interface SimFlag { kind: string; text: string; seed: number; matches: number }
export interface SimSummary {
  asked: number;
  ran: number;
  wins: Array<{ winner: SimWinner; matches: number }>;
  ends: Array<{ end: SimEnd; matches: number }>;
  avg_score: number;
  max_score: number;
  avg_ticks: number;
  max_ticks: number;
  worst_tick_ops: number;
  worst_tick_seed: number;
  budget: number;
  errors: Array<{ seed: number; message: string }>;
  flags: SimFlag[];
  stopped: string | null;
}
/** One generated match in full: the scenario it made and its trace. */
export interface SimMatch { seed: number; end: SimEnd; why: string | null; winner: SimWinner; top_score: number; scenario: ScenarioDraft; trace: Trace }
export interface SimResult { ok: true; teams: number; summary: SimSummary | null; rows: SimRow[]; match: SimMatch }

const ENDS: ReadonlyArray<SimEnd> = ["script", "score_limit", "time_limit", "error", "cap"];
const simEnd = (v: unknown): SimEnd => (ENDS.includes(v as SimEnd) ? (v as SimEnd) : "cap");
function simWinner(v: unknown): SimWinner {
  const w = obj(v);
  const kind = w.kind === "player" || w.kind === "team" || w.kind === "draw" ? w.kind : "nobody";
  return { kind, index: typeof w.index === "number" ? w.index : null };
}

/** `POST /v1/rules/simulate`'s answer. */
export function parseSim(body: unknown): SimResult | RunFailed {
  const b = obj(body);
  if (b.ok !== true && Array.isArray(b.errors)) return { ok: false, errors: parseProblems(b.errors) };
  const m = obj(b.match), sum = b.summary && typeof b.summary === "object" ? obj(b.summary) : null;
  const trace = parseTrace(m.trace);
  return {
    ok: true,
    teams: num(b.teams),
    summary: sum && {
      asked: num(sum.asked),
      ran: num(sum.ran),
      wins: arr(sum.wins).map(obj).map((w) => ({ winner: simWinner(w.winner), matches: num(w.matches) })),
      ends: arr(sum.ends).map(obj).map((e) => ({ end: simEnd(e.end), matches: num(e.matches) })),
      avg_score: num(sum.avg_score),
      max_score: num(sum.max_score),
      avg_ticks: num(sum.avg_ticks),
      max_ticks: num(sum.max_ticks),
      worst_tick_ops: num(sum.worst_tick_ops),
      worst_tick_seed: num(sum.worst_tick_seed),
      budget: num(sum.budget),
      errors: arr(sum.errors).map(obj).map((e) => ({ seed: num(e.seed), message: str(e.message) })),
      flags: arr(sum.flags).map(obj).map((f) => ({ kind: str(f.kind), text: str(f.text), seed: num(f.seed), matches: num(f.matches) })),
      stopped: optStr(sum.stopped),
    },
    rows: arr(b.rows).map(obj).map((r) => ({
      seed: num(r.seed), end: simEnd(r.end), winner: simWinner(r.winner), ticks: num(r.ticks), top_score: num(r.top_score), ops: num(r.ops), worst_tick_ops: num(r.worst_tick_ops),
    })),
    match: {
      seed: num(m.seed),
      end: simEnd(m.end),
      why: optStr(m.why),
      winner: simWinner(m.winner),
      top_score: num(m.top_score),
      scenario: draftOfWire(m.scenario),
      trace: runFailed(trace) ? parseTrace({}) as Trace : trace,
    },
  };
}

export const simFailed = (r: SimResult | RunFailed): r is RunFailed => "errors" in r;

/** A winner as the page names one: "Player 3", "Red team", "Draw". */
export function winnerName(w: SimWinner, teamLabel: (team: number) => string = (n) => `Team ${n}`): string {
  if (w.kind === "player") return playerName(w.index ?? 0);
  if (w.kind === "team") return teamLabel(w.index ?? 0);
  return w.kind === "draw" ? "Draw" : "Nobody";
}

/** How a match ended, as a label. */
export const endLabel = (e: SimEnd): string => ({ script: "Script ended it", score_limit: "Score limit", time_limit: "Time limit", error: "Script error", cap: "Never ended" })[e];

/** A count as a share of the matches played: "9 of 25". */
export const ofMatches = (n: number, ran: number): string => `${n} of ${ran}`;

/** Minutes and seconds, for a length: tick 5700 is "3:10". */
export function shortTime(tick: number): string {
  const s = Math.round(Math.max(0, tick) / TICKS_PER_SECOND);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** One thing a batch found worth a look, with the seed that shows it. */
export interface SimWarning { text: string; seed: number | null; bad: boolean }

/** A batch's warnings in the order they matter: script errors, then the
 *  sanity flags, then a batch cut short. Each starts with a capital. */
export function simWarnings(s: SimSummary): SimWarning[] {
  const out: SimWarning[] = s.errors.map((e) => ({ text: capital(e.message), seed: e.seed, bad: true }));
  for (const f of s.flags) out.push({ text: capital(f.text), seed: f.seed, bad: f.kind === "never_ends" || f.kind === "negative_score" });
  if (s.stopped) out.push({ text: capital(s.stopped), seed: null, bad: false });
  return out;
}

/** The seed before or after `seed` among a batch's rows; null at an end
 *  or when the seed is not one of them. */
export function neighbourSeed(rows: SimRow[], seed: number, step: -1 | 1): number | null {
  const i = rows.findIndex((r) => r.seed === seed);
  return i < 0 ? null : rows[i + step]?.seed ?? null;
}

// ---------- failures and gates ----------

/** Whose fault a failed request is. */
export type Fault = "server" | "script" | "network" | "account";
export interface Trouble {
  fault: Fault;
  /** What happened and what to do, as sentences. */
  text: string;
  /** The server has no rules routes at all. */
  missing: boolean;
  /** Whether asking again can help. */
  retry: boolean;
  /** The server didn't answer, or answered that it is broken. */
  down: boolean;
}

export const TOOLS_MISSING = "This Live server doesn't have the script tools yet.";
const TOOLS_DOWN = "The Live server isn't answering.";

const sentence = (s: string): string => {
  const t = s.trim();
  if (!t) return "";
  return t[0]!.toUpperCase() + t.slice(1) + (/[.!?]$/.test(t) ? "" : ".");
};

/** A failed request in words. `route` says the request was to a rules
 *  route that every server with the script tools has (check, run, save,
 *  the variant routes), so "not found" means the server lacks them; a
 *  script opened by hash can be not found on any server. */
export function explainFailure(e: unknown, route = true): Trouble {
  if (!(e instanceof ApiError)) {
    return { fault: "network", text: "Can't reach the Live server. Your script isn't the cause; check the connection and try again.", missing: false, retry: true, down: true };
  }
  const noRoute = e.status === 405 || e.status === 501 || (e.status === 404 && (route || /no such endpoint/i.test(e.message)));
  if (noRoute) {
    return { fault: "server", text: `${TOOLS_MISSING} You can still write here and your draft is kept, but checking, testing and saving need a newer server.`, missing: true, retry: false, down: false };
  }
  if (e.status === 404) return { fault: "script", text: `The server couldn't find it. ${sentence(e.message)}`, missing: false, retry: false, down: false };
  if (e.status === 413) return { fault: "script", text: `The script is too long. A script is at most ${MAX_SOURCE / 1024} KB.`, missing: false, retry: false, down: false };
  if (e.status === 429) return { fault: "server", text: "The server is limiting requests. Your script isn't the cause; wait a few seconds.", missing: false, retry: true, down: false };
  if (e.status === 401 || e.status === 403) return { fault: "account", text: "The server didn't accept your sign-in. Sign out and back in, then try again.", missing: false, retry: false, down: false };
  if (e.status >= 500) return { fault: "server", text: `The Live server had a problem (HTTP ${e.status}). Your script isn't the cause; try again.`, missing: false, retry: true, down: true };
  return { fault: "script", text: `The server refused the request. ${sentence(e.message)}`, missing: false, retry: false, down: false };
}

/** A fault as the label before its words. */
export const faultLabel = (f: Fault): string => ({ server: "Server", script: "Script", network: "Connection", account: "Account" })[f];

/** Whether the server's script tools answer. */
export type Tools = "unknown" | "ok" | "missing" | "down";

/** What a failed check says about the tools; null when nothing. */
export function toolsAfter(t: Trouble): Tools | null {
  return t.missing ? "missing" : t.down ? "down" : null;
}

/** "1 problem", "3 problems". */
export const problemCount = (n: number): string => `${n} ${n === 1 ? "problem" : "problems"}`;

/** What the buttons depend on. */
export interface Gate { tools: Tools; check: CheckResult | null; locked: boolean }

function scriptBlock(g: Gate): string | null {
  if (g.tools === "missing") return TOOLS_MISSING;
  if (g.tools === "down") return TOOLS_DOWN;
  if (!g.check) return "The script is still being checked.";
  if (g.check.ok) return null;
  return g.check.errors.length ? `Fix the ${problemCount(g.check.errors.length)} in the script first.` : "The script doesn't verify yet.";
}

/** Why Save is off; null when it is on. */
export function saveBlocked(g: Gate): string | null {
  if (g.tools === "missing") return TOOLS_MISSING;
  if (g.locked) return "This script is read-only. Choose Use as start to make your own copy.";
  return scriptBlock(g);
}

/** Why Make a game variant is off. */
export function variantBlocked(g: Gate): string | null {
  return scriptBlock(g) ?? (g.check?.hash ? null : "The check gave this script no hash.");
}

/** Why the scenario can't run. */
export function runBlocked(g: Gate, problems: string[]): string | null {
  return scriptBlock(g) ?? (problems.length ? `Fix the ${problemCount(problems.length)} in the scenario first.` : null);
}

/** The buttons that are off, those with one reason together: "Save and
 *  Make a game variant are off." and the reason. */
export function whyOff(buttons: Array<[label: string, reason: string | null]>): Array<{ what: string; reason: string }> {
  const by = new Map<string, string[]>();
  for (const [label, reason] of buttons) if (reason) by.set(reason, [...(by.get(reason) ?? []), label]);
  return [...by.entries()].map(([reason, labels]) => ({
    what: labels.length === 1 ? `${labels[0]} is off.` : `${labels.slice(0, -1).join(", ")} and ${labels[labels.length - 1]} are off.`,
    reason,
  }));
}

// ---------- drafts ----------

export const DRAFT_KEY = "h2.rules.draft";
/** What the page keeps: the script, the generator's setup, and the match
 *  edited by hand with whether it is the one in use (`custom`). */
export interface Draft { source: string; scenario: ScenarioDraft; setup: SimSetup; custom: boolean }

export function loadDraft(storage: Pick<Storage, "getItem"> | null, fallback: string): Draft {
  try {
    const raw = storage?.getItem(DRAFT_KEY);
    if (raw) {
      const b = obj(JSON.parse(raw));
      if (typeof b.source === "string") return { source: b.source, scenario: parseScenario(b.scenario), setup: parseSetup(b.setup), custom: b.custom === true };
    }
  } catch { /* unreadable: start over */ }
  return { source: fallback, scenario: defaultScenario(), setup: defaultSetup(), custom: false };
}

export function saveDraft(storage: Pick<Storage, "setItem"> | null, d: Draft): void {
  try { storage?.setItem(DRAFT_KEY, JSON.stringify(d)); } catch { /* private mode or full */ }
}

// ---------- the API ----------

/** A script `GET /v1/rules/builtin` lists: an engine port, or an example
 *  (`example`, with its `key`) that rides `engine` as its carrier. */
export interface Builtin { name: string; engine: number; source: string; hash: string | null; example: boolean; key: string | null }
export interface ScriptSummary { hash: string; name: string; version: number; author: string; gamertag: string; saved_at: string }
export interface Script extends ScriptSummary { source: string }

export function parseBuiltins(body: unknown): Builtin[] {
  return arr(obj(body).items).map(obj).filter((b) => typeof b.source === "string").map((b) => ({ name: str(b.name) || "Built-in", engine: num(b.engine), source: str(b.source), hash: optStr(b.hash), example: b.example === true, key: optStr(b.key) }));
}

const summaryOf = (s: Record<string, unknown>): ScriptSummary => ({
  hash: str(s.hash), name: str(s.name), version: num(s.version), author: str(s.author), gamertag: str(s.gamertag), saved_at: str(s.saved_at),
});

export function parseScripts(body: unknown): ScriptSummary[] {
  return arr(obj(body).items).map(obj).filter((s) => /^[0-9a-f]{64}$/.test(str(s.hash))).map(summaryOf);
}

export function parseScript(body: unknown): Script | null {
  const s = obj(body);
  return /^[0-9a-f]{64}$/.test(str(s.hash)) && typeof s.source === "string" ? { ...summaryOf(s), source: s.source } : null;
}

async function answer(r: Response): Promise<unknown> {
  if (!r.ok) {
    let code = "http_" + r.status, message = r.statusText || `HTTP ${r.status}`;
    try {
      const b = await r.json();
      if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; }
    } catch { /* not JSON */ }
    throw new ApiError(r.status, code, message);
  }
  return r.json();
}

const postJson = (fetchFn: typeof fetch, url: string, body: unknown, headers: Record<string, string> = {}, signal?: AbortSignal) =>
  fetchFn(url, { method: "POST", signal, headers: { accept: "application/json", "content-type": "application/json", ...headers }, body: JSON.stringify(body) });

export async function checkRules(api: string, source: string, fetchFn: typeof fetch = fetch, signal?: AbortSignal): Promise<CheckResult> {
  return parseCheck(await answer(await postJson(fetchFn, `${api}/rules/check`, { source }, {}, signal)));
}

export async function runRules(api: string, source: string, scenario: ScenarioDraft, fetchFn: typeof fetch = fetch, signal?: AbortSignal): Promise<Trace | RunFailed> {
  return parseTrace(await answer(await postJson(fetchFn, `${api}/rules/run`, { source, scenario: buildScenario(scenario) }, {}, signal)));
}

/** Plays `matches` generated matches from `seed` (the setup's when not
 *  given); the answer's `match` is `seed`'s, in full. */
export async function simulateRules(api: string, source: string, setup: SimSetup, matches: number, seed: number = setup.seed, fetchFn: typeof fetch = fetch, signal?: AbortSignal): Promise<SimResult | RunFailed> {
  return parseSim(await answer(await postJson(fetchFn, `${api}/rules/simulate`, simRequest(source, setup, matches, seed), {}, signal)));
}

export async function fetchBuiltins(api: string, fetchFn: typeof fetch = fetch): Promise<Builtin[]> {
  return parseBuiltins(await answer(await fetchFn(`${api}/rules/builtin`, { headers: { accept: "application/json" } })));
}

export async function fetchScripts(api: string, author: string, fetchFn: typeof fetch = fetch): Promise<ScriptSummary[]> {
  return parseScripts(await answer(await fetchFn(`${api}/rules?author=${encodeURIComponent(author)}`, { headers: { accept: "application/json" } })));
}

export async function fetchScript(api: string, hash: string, fetchFn: typeof fetch = fetch): Promise<Script | null> {
  return parseScript(await answer(await fetchFn(`${api}/rules/${encodeURIComponent(hash)}`, { headers: { accept: "application/json" } })));
}

/** Saves a script that verifies; whether this saved it (false: the hash
 *  was already saved, by anyone). */
export async function saveRules(api: string, aud: string, me: LocalAccount, source: string, fetchFn: typeof fetch = fetch): Promise<{ saved: boolean; script: Script | null }> {
  const token = await issueToken(me, aud, CONTENT_WRITE);
  const b = obj(await answer(await postJson(fetchFn, `${api}/rules`, { source }, { authorization: `Bearer ${token}` })));
  return { saved: b.saved === true, script: parseScript(b.script) };
}
