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
// - `POST /v1/rules` (a `content.write` token), `GET /v1/rules/{hash}`,
//   `GET /v1/rules?author=`: kept in memory.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { parseJws } from "../src/state/verify";
import type { Reply } from "./admin";
import type { World } from "./world";

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

interface Saved { hash: string; name: string; version: number; author: string; gamertag: string; saved_at: string; position: number; source: string }

const summary = (s: Saved) => ({ hash: s.hash, name: s.name, version: s.version, author: s.author, gamertag: s.gamertag, saved_at: s.saved_at, position: s.position });

export class MockRules {
  readonly saved = new Map<string, Saved>();

  constructor(private world: World) {}

  async handle(req: Request, u: URL): Promise<Reply | null> {
    const path = u.pathname;
    if (!path.startsWith("/v1/rules")) return null;
    const body = async (): Promise<Record<string, unknown> | null> => {
      try { const b = await req.json(); return b && typeof b === "object" ? b as Record<string, unknown> : null; } catch { return null; }
    };
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
    if (path === "/v1/rules/builtin" && req.method === "GET") {
      const source = slayer();
      return ok({ items: [{ name: "Slayer", engine: 2, source, hash: sha(source) }] });
    }
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
