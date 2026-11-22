// The game type script workbench (docs/game/rules-engine.md "Workbench",
// docs/live/live-api.md "Rules scripts"): answers read defensively, the
// scenario built into the run request, the trace as lines, errors mapped
// onto the text, the editor's tab key, drafts, the calls with their token
// scope, times as typed, the scenario's quick edits, the results as a
// timeline and standings, failures in words, why a button is off, the
// generated matches (the setup, the request, the answer, its warnings),
// and the mock's routes. Every gamertag and script is invented.

import { describe, expect, test } from "bun:test";
import {
  CONTENT_WRITE, DRAFT_KEY, INDENT, MAX_SEED, SIM_MATCHES, SPAWNED_BASE, TOOLS_MISSING, UNIT_BASE, buildScenario, checkRules,
  defaultScenario, defaultSetup, describeEvent, draftOfWire, endLabel, explainFailure, faultLabel, formatCall, gutter, lineAt,
  loadDraft, meter, moveEvent, neighbourSeed, newEvent, nextEventTick, nextSlot, objectName, ofMatches, offsetOf, parseBuiltins, parseCheck,
  parseSetup, parseSim, parseTickTime, parseTrace, problemCount, problemPlace, rerolled, runBlocked, runFailed, runRules,
  saveBlocked, saveDraft, saveRules, scenarioProblems, shortTime, signed, simFailed, simRequest, simWarnings, simulateRules,
  sortEvents, standings, tabKey, teamStandings, tickTime, timeline, toolsAfter, traceLines, variantBlocked, whyOff, winnerName,
  wireEvent, withEvent, type CheckResult, type Gate, type ScenarioDraft, type SimResult, type Trace,
} from "../src/lib/rules";
import { ApiError } from "../src/api/http";
import type { LocalAccount } from "../src/state/account";
import { parseJws } from "../src/state/verify";

const RFC = { d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo", kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k" };
const me = (): LocalAccount => ({ xuid: "0009000000000042", gamertag: "afr", id: "acct:0009000000000042@localhost", server: "http://127.0.0.1:8074", server_id: "localhost", kid: RFC.kid, key: RFC.d, x: RFC.x });

function recorder(reply: (url: string, init: RequestInit) => Response) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = (async (url: string, init: RequestInit = {}) => { calls.push({ url, init }); return reply(url, init); }) as unknown as typeof fetch;
  return { f, calls };
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const SRC = 'rules "Test" 1\ncarrier slayer\non kill(v, k) {\n    score(k, 1)\n}\n';

describe("reading a check", () => {
  test("a full answer", () => {
    const c = parseCheck({
      ok: true, hash: "ab".repeat(32), name: "Test", version: 1, errors: [],
      report: {
        worst_tick_ops: 96, budget: 20000, fits: true, costliest: "kill", handlers: [{ event: "kill", worst_ops: 6 }],
        slots: [{ scope: "global", used: 1, limit: 64 }], ir_bytes: 120, ir_limit: 65536,
        capabilities: [{ name: "player.team", words: "moves players between teams", grantable: false, known: true }], unhostable: ["set_team"],
      },
    });
    expect(c.ok).toBe(true);
    expect(c.report!.handlers).toEqual([{ event: "kill", worst_ops: 6 }]);
    expect(c.report!.capabilities[0]!.words).toBe("moves players between teams");
    expect(c.report!.unhostable).toEqual(["set_team"]);
  });

  test("missing and malformed fields read as nothing", () => {
    const c = parseCheck({ ok: "yes", errors: [{ line: "x", message: 3 }, { line: 4, col: 2, message: "expected )" }], report: null });
    expect(c.ok).toBe(false);
    expect(c.hash).toBeNull();
    expect(c.errors).toEqual([{ line: 0, col: 0, message: "error" }, { line: 4, col: 2, message: "expected )" }]);
    expect(c.report).toBeNull();
    expect(parseCheck(null).errors).toEqual([]);
  });

  test("places and meters", () => {
    expect(problemPlace({ line: 3, col: 5, message: "" })).toBe("line 3, col 5");
    expect(problemPlace({ line: 0, col: 0, message: "" })).toBe("script");
    expect(meter(10, 100)).toEqual({ share: 0.1, tone: "ok" });
    expect(meter(80, 100).tone).toBe("warn");
    expect(meter(120, 100)).toEqual({ share: 1, tone: "over" });
    expect(meter(5, 0)).toEqual({ share: 0, tone: "ok" });
  });
});

describe("the editor", () => {
  test("errors map onto the text, clamped to their line", () => {
    expect(offsetOf(SRC, 4, 5)).toBe(SRC.indexOf("score"));
    expect(offsetOf(SRC, 1, 1)).toBe(0);
    expect(offsetOf(SRC, 1, 999)).toBe(SRC.indexOf("\n"));
    expect(offsetOf(SRC, 99, 1)).toBe(SRC.length);
    expect(offsetOf(SRC, 0, 0)).toBe(0);
    expect(lineAt(SRC, SRC.indexOf("score"))).toBe(4);
  });

  test("the gutter numbers every line", () => {
    expect(gutter("a\nb\nc")).toBe("1\n2\n3");
    expect(gutter("")).toBe("1");
  });

  test("tab inserts spaces at the caret", () => {
    expect(tabKey("ab", 1, 1, false)).toEqual({ value: `a${INDENT}b`, start: 1 + INDENT.length, end: 1 + INDENT.length });
  });

  test("tab and shift-tab indent and outdent the selected lines", () => {
    const v = "one\ntwo\nthree";
    const r = tabKey(v, 0, 7, false);
    expect(r.value).toBe(`${INDENT}one\n${INDENT}two\nthree`);
    expect(r.start).toBe(INDENT.length);
    const back = tabKey(r.value, r.start, r.end, true);
    expect(back.value).toBe(v);
    expect(tabKey("  x", 3, 3, true).value).toBe("x");
  });
});

describe("scenarios", () => {
  test("events group by tick, in tick order, keeping their listed order", () => {
    const d: ScenarioDraft = {
      ...defaultScenario(),
      events: [
        { tick: 9, type: "kill", victim: 0, killer: null },
        { tick: 3, type: "kill", victim: 1, killer: 0 },
        { tick: 9, type: "spawn", player: 1 },
      ],
      ticks: 30, seed: 7, roundTime: 100, teamPlay: true,
    };
    expect(buildScenario(d)).toEqual({
      players: [{ slot: 0, team: 0, lives: 0 }, { slot: 1, team: 1, lives: 0 }],
      variant: { team_play: 1 },
      seed: 7,
      round_time: 100,
      ticks: 30,
      start: true,
      timeline: [
        { tick: 3, events: [{ type: "kill", victim: 1, killer: 0 }] },
        { tick: 9, events: [{ type: "kill", victim: 0, killer: null }, { type: "spawn", player: 1 }] },
      ],
    });
  });

  test("an event carries only its own fields", () => {
    expect(wireEvent({ tick: 1, type: "enter", zone: 2, object: 5, player: 3 })).toEqual({ type: "enter", zone: 2, object: 5 });
    expect(wireEvent({ tick: 1, type: "round_start", player: 1 })).toEqual({ type: "round_start" });
    expect(wireEvent({ tick: 1, type: "kill", victim: 1 })).toEqual({ type: "kill", victim: 1, killer: null });
  });

  test("new events and players come from the seated ones", () => {
    const players = [{ slot: 2, team: 0, lives: 0 }, { slot: 5, team: 1, lives: 0 }];
    expect(newEvent("kill", 30, players)).toEqual({ tick: 30, type: "kill", victim: 5, killer: 2 });
    expect(newEvent("press", 4, players)).toEqual({ tick: 4, type: "press", player: 2, object: 0 });
    expect(nextSlot(players)).toBe(0);
    expect(nextSlot(Array.from({ length: 16 }, (_, slot) => ({ slot, team: 0, lives: 0 })))).toBeNull();
  });

  test("problems are named before the run", () => {
    const d = defaultScenario();
    expect(scenarioProblems(d)).toEqual([]);
    expect(scenarioProblems({ ...d, ticks: 40_000 })).toContain("run between 0 and 36000 ticks");
    expect(scenarioProblems({ ...d, events: [{ tick: 400, type: "spawn", player: 0 }] })[0]).toContain("past the last tick");
    expect(scenarioProblems({ ...d, events: [{ tick: 4, type: "death", player: 9 }] })[0]).toContain("player 9, who is not seated");
    expect(scenarioProblems({ ...d, events: [{ tick: 1, type: "join", player: 9 }, { tick: 4, type: "death", player: 9 }] })).toEqual([]);
  });
});

describe("traces", () => {
  const body = {
    ok: true, hash: "ab".repeat(32), options: 1, last_tick: 30, error: null, round_ended: null, cut: null, ops: 12,
    scores: [{ slot: 0, score: 1 }],
    ticks: [{
      tick: 75, ran: ["kill"], calls: [{ entry: "score", args: ["player 0", "1"] }], ops: 6, warnings: [],
      globals: [{ name: "wave", value: "2" }], players: [{ slot: 0, team: 0, alive: true, score: 1, lives: 0, vars: [] }], error: null,
    }],
  };

  test("a trace reads back, and a failed build reads as its errors", () => {
    const t = parseTrace(body);
    expect(runFailed(t)).toBe(false);
    expect((t as Trace).ticks[0]!.calls[0]).toEqual({ entry: "score", args: ["player 0", "1"] });
    const f = parseTrace({ ok: false, errors: [{ line: 2, col: 1, message: "nope" }] });
    expect(runFailed(f)).toBe(true);
  });

  test("ticks read as match time, calls as the script writes them", () => {
    expect(tickTime(0)).toBe("0:00.00");
    expect(tickTime(75)).toBe("0:02.50");
    expect(tickTime(1830)).toBe("1:01.00");
    expect(formatCall({ entry: "end_round", args: ["team 1"] })).toBe("end_round(team 1)");
  });

  test("the text view is one line a tick and how it ended", () => {
    const t = parseTrace(body) as Trace;
    expect(traceLines(t)).toEqual([
      "tick 75 (0:02.50) | ran on kill | score(player 0, 1) | wave = 2",
      "ran to tick 30",
    ]);
    expect(traceLines({ ...t, ticks: [], round_ended: 12, cut: "cut short" })).toEqual(["the round ended at tick 12", "cut short"]);
  });
});

describe("times as typed", () => {
  test("minutes and seconds, seconds or ticks", () => {
    expect(parseTickTime("0:02.50")).toBe(75);
    expect(parseTickTime("1:01")).toBe(1830);
    expect(parseTickTime("2.5")).toBe(75);
    expect(parseTickTime(" 2.5s ")).toBe(75);
    expect(parseTickTime(".5")).toBe(15);
    expect(parseTickTime("75t")).toBe(75);
    expect(parseTickTime("75 ticks")).toBe(75);
    expect(parseTickTime("0")).toBe(0);
  });

  test("what reads as no time is null", () => {
    for (const bad of ["", "soon", "-1", "1:75", "1:2:3", "2,5"]) expect(parseTickTime(bad)).toBeNull();
  });

  test("every tick's shown time reads back as that tick", () => {
    for (const tick of [0, 1, 29, 31, 75, 1799, 1830, 35_999]) expect(parseTickTime(tickTime(tick))).toBe(tick);
  });
});

describe("quick edits to a scenario", () => {
  test("a new event lands a second after the last, in tick order", () => {
    const d = defaultScenario();
    expect(nextEventTick(d)).toBe(120);
    expect(nextEventTick({ ...d, events: [] })).toBe(30);
    const next = withEvent(d, { tick: 45, type: "spawn", player: 0 });
    expect(next.events.map((e) => e.tick)).toEqual([30, 45, 60, 90]);
    expect(next.ticks).toBe(d.ticks);
  });

  test("an event past the end lengthens the run, up to the cap", () => {
    const d = defaultScenario();
    expect(withEvent(d, { tick: 400, type: "round_end" }).ticks).toBe(430);
    expect(moveEvent(d, 0, 900).ticks).toBe(930);
    expect(moveEvent(d, 0, 900).events.map((e) => e.tick)).toEqual([60, 90, 900]);
    expect(withEvent(d, { tick: 99_999, type: "round_end" }).ticks).toBe(36_000);
    expect(scenarioProblems(withEvent(d, { tick: 99_999, type: "round_end" }))).toEqual([]);
  });

  test("events of one tick keep their listed order", () => {
    const a = { tick: 5, type: "spawn" as const, player: 0 }, b = { tick: 5, type: "death" as const, player: 1 }, c = { tick: 1, type: "round_start" as const };
    expect(sortEvents([a, b, c])).toEqual([c, a, b]);
  });

  test("an event reads as a sentence", () => {
    expect(describeEvent({ tick: 0, type: "kill", victim: 1, killer: 0 })).toBe("Player 0 killed Player 1");
    expect(describeEvent({ tick: 0, type: "kill", victim: 1, killer: null })).toBe("The world killed Player 1");
    expect(describeEvent({ tick: 0, type: "kill", victim: 1, killer: 1 })).toBe("Player 1 killed themselves");
    expect(describeEvent({ tick: 0, type: "pickup", player: 2, object: 4 })).toBe("Player 2 picked up object 4");
    expect(describeEvent({ tick: 0, type: "enter", zone: 1, object: 3 })).toBe("Object 3 entered zone 1");
    expect(describeEvent({ tick: 0, type: "round_end" })).toBe("The round ended");
  });
});

describe("results", () => {
  const row = (slot: number, team: number, score: number, lives = 0, vars: Array<{ name: string; value: string }> = []) => ({ slot, team, alive: true, score, lives, vars });
  const trace: Trace = {
    ok: true, hash: "ab".repeat(32), last_tick: 300, error: null, round_ended: null, cut: null, ops: 24,
    scores: [{ slot: 0, score: 0 }, { slot: 1, score: 0 }],
    ticks: [
      { tick: 30, ran: ["kill"], calls: [{ entry: "score", args: ["player 0", "1"] }], ops: 12, warnings: [], globals: [{ name: "wave", value: "1" }], players: [row(0, 0, 1), row(1, 1, 0)], error: null },
      { tick: 90, ran: ["kill"], calls: [{ entry: "score", args: ["player 0", "-1"] }], ops: 12, warnings: ["slow"], globals: [{ name: "wave", value: "1" }], players: [row(0, 0, 0), row(1, 0, 0, 2, [{ name: "it", value: "true" }])], error: null },
    ],
  };

  test("the timeline joins the scenario's events to what the script did", () => {
    const rows = timeline(trace, defaultScenario(), (n) => `the ${["Red", "Blue"][n]} team`);
    expect(rows.map((r) => r.tick)).toEqual([30, 60, 90]);
    expect(rows[0]).toEqual({
      tick: 30, happened: ["Player 0 killed Player 1"], ran: ["kill"], calls: ["score(player 0, 1)"],
      scored: [{ slot: 0, delta: 1, score: 1 }], changes: ["wave = 1"], warnings: [], error: null,
    });
    // A spawn no handler answered is still a moment of the run.
    expect(rows[1]).toEqual({ tick: 60, happened: ["Player 1 spawned"], ran: [], calls: [], scored: [], changes: [], warnings: [], error: null });
    expect(rows[2]!.scored).toEqual([{ slot: 0, delta: -1, score: 0 }]);
    // A variable that kept its value is not a change; a team, lives and a new variable are.
    expect(rows[2]!.changes).toEqual(["Player 1 moved to the Red team", "Player 1 has 2 lives", "Player 1: it = true"]);
    expect(rows[2]!.warnings).toEqual(["slow"]);
  });

  test("the timeline stops where the round ended", () => {
    const rows = timeline({ ...trace, round_ended: 30, ticks: trace.ticks.slice(0, 1) }, defaultScenario());
    expect(rows.map((r) => r.tick)).toEqual([30]);
    expect(timeline({ ...trace, ticks: [] }, { ...defaultScenario(), events: [] })).toEqual([]);
  });

  test("standings rank by score, ties sharing a place, with the last team seen", () => {
    const t = { ...trace, scores: [{ slot: 0, score: 2 }, { slot: 1, score: 5 }, { slot: 2, score: 2 }] };
    const rows = standings(t, { ...defaultScenario(), players: [{ slot: 0, team: 0, lives: 0 }, { slot: 1, team: 1, lives: 0 }, { slot: 2, team: 1, lives: 0 }] });
    expect(rows).toEqual([
      { place: 1, slot: 1, team: 0, score: 5 },
      { place: 2, slot: 0, team: 0, score: 2 },
      { place: 2, slot: 2, team: 1, score: 2 },
    ]);
    expect(teamStandings(rows)).toEqual([{ place: 1, team: 0, score: 7 }, { place: 2, team: 1, score: 2 }]);
    expect(signed(2)).toBe("+2");
    expect(signed(-1)).toBe("-1");
  });
});

describe("failures in words", () => {
  test("a server without the routes is named as that, not as the script's fault", () => {
    const t = explainFailure(new ApiError(404, "not_found", "no such endpoint"));
    expect(t.missing).toBe(true);
    expect(t.fault).toBe("server");
    expect(t.text).toContain(TOOLS_MISSING);
    expect(t.text).not.toContain("no such endpoint");
    expect(toolsAfter(t)).toBe("missing");
    expect(explainFailure(new ApiError(405, "http_405", "Method Not Allowed")).missing).toBe(true);
    // Opening a script by hash: the route exists, the script doesn't.
    const gone = explainFailure(new ApiError(404, "not_found", "no such script"), false);
    expect(gone.missing).toBe(false);
    expect(gone.fault).toBe("script");
    expect(explainFailure(new ApiError(404, "not_found", "no such endpoint"), false).missing).toBe(true);
  });

  test("the server's own trouble and the network's are not the script's", () => {
    const down = explainFailure(new TypeError("Failed to fetch"));
    expect(down.fault).toBe("network");
    expect(down.retry).toBe(true);
    expect(toolsAfter(down)).toBe("down");
    const broke = explainFailure(new ApiError(503, "http_503", "Service Unavailable"));
    expect(broke.fault).toBe("server");
    expect(broke.text).toContain("HTTP 503");
    expect(toolsAfter(broke)).toBe("down");
    const limited = explainFailure(new ApiError(429, "rate_limited", "too many checks"));
    expect(limited.fault).toBe("server");
    expect(limited.retry).toBe(true);
    // A limit is no sign the server is down.
    expect(toolsAfter(limited)).toBeNull();
  });

  test("a refusal of the script carries the server's words as a sentence", () => {
    const long = explainFailure(new ApiError(413, "too_large", "a script's source is at most 262144 bytes"));
    expect(long.fault).toBe("script");
    expect(long.text).toContain("256 KB");
    expect(toolsAfter(long)).toBeNull();
    expect(explainFailure(new ApiError(400, "invalid", "scenario: a scenario runs at most 36000 ticks")).text)
      .toBe("The server refused the request. Scenario: a scenario runs at most 36000 ticks.");
    expect(explainFailure(new ApiError(401, "unauthorized", "this needs a content.write token")).fault).toBe("account");
    expect(faultLabel("network")).toBe("Connection");
  });
});

describe("why a button is off", () => {
  const ok: CheckResult = { ok: true, hash: "ab".repeat(32), name: "Test", version: 1, errors: [], report: null, options: [], carrier: null };
  const bad: CheckResult = { ...ok, ok: false, hash: null, errors: [{ line: 1, col: 1, message: "x" }, { line: 2, col: 1, message: "y" }] };
  const gate = (over: Partial<Gate> = {}): Gate => ({ tools: "ok", check: ok, locked: false, ...over });

  test("nothing is off for a script that verifies", () => {
    expect(saveBlocked(gate())).toBeNull();
    expect(variantBlocked(gate())).toBeNull();
    expect(runBlocked(gate(), [])).toBeNull();
  });

  test("each state gives its reason", () => {
    expect(saveBlocked(gate({ check: bad }))).toBe("Fix the 2 problems in the script first.");
    expect(saveBlocked(gate({ check: { ...bad, errors: [bad.errors[0]!] } }))).toBe("Fix the 1 problem in the script first.");
    expect(saveBlocked(gate({ check: null }))).toBe("The script is still being checked.");
    expect(saveBlocked(gate({ locked: true }))).toContain("read-only");
    expect(saveBlocked(gate({ tools: "missing", check: null, locked: true }))).toBe(TOOLS_MISSING);
    expect(variantBlocked(gate({ tools: "down", check: null }))).toBe("The Live server isn't answering.");
    // A read-only script can still be made into a variant and run.
    expect(variantBlocked(gate({ locked: true }))).toBeNull();
    expect(variantBlocked(gate({ check: { ...ok, hash: null } }))).toContain("no hash");
    expect(runBlocked(gate(), ["two players share a slot"])).toBe("Fix the 1 problem in the scenario first.");
    expect(runBlocked(gate({ check: bad }), ["two players share a slot"])).toBe("Fix the 2 problems in the script first.");
    expect(problemCount(1)).toBe("1 problem");
  });

  test("buttons with one reason are named together", () => {
    expect(whyOff([["Save", null], ["Make a game variant", null]])).toEqual([]);
    expect(whyOff([["Save", "Fix it."], ["Make a game variant", "Fix it."]])).toEqual([{ what: "Save and Make a game variant are off.", reason: "Fix it." }]);
    expect(whyOff([["Save", "It is read-only."], ["Make a game variant", null]])).toEqual([{ what: "Save is off.", reason: "It is read-only." }]);
    expect(whyOff([["A", "x"], ["B", "x"], ["C", "x"]])[0]!.what).toBe("A, B and C are off.");
  });
});

describe("drafts", () => {
  test("a draft round-trips, and an unreadable one starts over", () => {
    const store = new Map<string, string>();
    const s = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) };
    expect(loadDraft(s, "fresh").source).toBe("fresh");
    const scenario = { ...defaultScenario(), ticks: 99 };
    expect(loadDraft(s, "fresh").setup).toEqual(defaultSetup());
    expect(loadDraft(s, "fresh").custom).toBe(false);
    const setup = { players: 6, teamPlay: true, seed: 77 };
    saveDraft(s, { source: SRC, scenario, setup, custom: true });
    expect(loadDraft(s, "fresh")).toEqual({ source: SRC, scenario, setup, custom: true });
    // A match opened from a generated one keeps its record and objects.
    const made = { ...scenario, variant: { team_play: 0, score_limit: 25 }, objects: [{ id: 256, label: "hill_corner", number: 0, team: null }] };
    saveDraft(s, { source: SRC, scenario: made, setup, custom: true });
    expect(loadDraft(s, "fresh").scenario).toEqual(made);
    store.set(DRAFT_KEY, "{not json");
    expect(loadDraft(s, "fresh").source).toBe("fresh");
    store.set(DRAFT_KEY, JSON.stringify({ source: "x", scenario: { events: [{ tick: 1, type: "fly" }], ticks: "ten" } }));
    const d = loadDraft(s, "fresh");
    expect(d.setup).toEqual(defaultSetup());
    expect(d.custom).toBe(false);
    expect(d.scenario.events).toEqual([]);
    expect(d.scenario.ticks).toBe(defaultScenario().ticks);
    expect(loadDraft(null, "fresh").source).toBe("fresh");
  });
});

describe("the calls", () => {
  test("check posts the source; save signs a content.write token", async () => {
    const { f, calls } = recorder((url) => json(url.endsWith("/check") ? { ok: true, hash: "ab".repeat(32), errors: [] } : { ok: true, saved: true, script: { hash: "ab".repeat(32), name: "Test", version: 1, source: SRC } }, 201));
    const c = await checkRules("http://x/v1", SRC, f);
    expect(c.ok).toBe(true);
    expect(calls[0]!.url).toBe("http://x/v1/rules/check");
    expect(JSON.parse(String(calls[0]!.init.body))).toEqual({ source: SRC });
    const r = await saveRules("http://x/v1", "localhost", me(), SRC, f);
    expect(r.saved).toBe(true);
    expect(r.script!.name).toBe("Test");
    const auth = (calls[1]!.init.headers as Record<string, string>).authorization!;
    expect((parseJws(auth.slice(7))!.payload as { lxm: string }).lxm).toBe(CONTENT_WRITE);
  });

  test("run posts the built scenario and passes its signal on", async () => {
    const { f, calls } = recorder(() => json({ ok: true, hash: "ab".repeat(32), ticks: [], last_tick: 300, scores: [] }));
    const stop = new AbortController();
    const t = await runRules("http://x/v1", SRC, defaultScenario(), f, stop.signal);
    expect(runFailed(t)).toBe(false);
    expect(calls[0]!.url).toBe("http://x/v1/rules/run");
    expect(JSON.parse(String(calls[0]!.init.body))).toEqual({ source: SRC, scenario: buildScenario(defaultScenario()) });
    expect(calls[0]!.init.signal).toBe(stop.signal);
  });

  test("a refusal is an ApiError with the server's message", async () => {
    const { f } = recorder(() => json({ error: "rate_limited", message: "too many checks" }, 429));
    await expect(checkRules("http://x/v1", SRC, f)).rejects.toThrow("too many checks");
  });
});

describe("the mock's workbench", async () => {
  const { MockRules, mockCheck, handlersOf } = await import("../mock/rules");
  const { World } = await import("../mock/world");

  test("check finds the header and unbalanced braces by line", () => {
    expect(mockCheck(SRC).ok).toBe(true);
    expect(mockCheck(SRC).report!.handlers).toEqual([{ event: "kill", worst_ops: 24 }]);
    expect(mockCheck("carrier slayer").errors[0]!.line).toBe(1);
    expect(mockCheck('rules "X" 1\non tick {\n').errors[0]!.message).toContain("never closed");
    expect(handlersOf("on timer(next) {}\non kill(a, b) {}")).toEqual(["timer(next)", "kill"]);
  });

  test("run scores kills; builtin lists Slayer", async () => {
    const m = new MockRules(new World());
    const r = await m.handle(new Request("http://m/v1/rules/run", { method: "POST", body: JSON.stringify({ source: SRC, scenario: buildScenario(defaultScenario()) }) }), new URL("http://m/v1/rules/run"));
    const t = parseTrace(r!.body) as Trace;
    expect(t.ticks[0]!.calls[0]).toEqual({ entry: "score", args: ["player 0", "1"] });
    const b = await m.handle(new Request("http://m/v1/rules/builtin"), new URL("http://m/v1/rules/builtin"));
    expect((b!.body as { items: Array<{ name: string }> }).items[0]!.name).toBe("Slayer");
    const listed = parseBuiltins(b!.body);
    expect(listed[0]).toMatchObject({ name: "Slayer", example: false, key: null });
    expect(parseBuiltins({ items: [{ name: "Infection", key: "infection", engine: 2, source: "rules", example: true }] })[0]).toMatchObject({ example: true, key: "infection" });
    expect(await m.handle(new Request("http://m/v1/other"), new URL("http://m/v1/other"))).toBeNull();
  });

  test("with the tools off every route is a server from before them", async () => {
    const m = new MockRules(new World(), false);
    for (const [path, method] of [["/v1/rules/check", "POST"], ["/v1/rules/run", "POST"], ["/v1/rules/simulate", "POST"], ["/v1/rules/builtin", "GET"], ["/v1/rules/ide", "POST"]] as const) {
      const r = await m.handle(new Request(`http://m${path}`, { method, body: method === "POST" ? JSON.stringify({ source: SRC }) : undefined }), new URL(`http://m${path}`));
      expect(r!.status).toBe(404);
      const body = r!.body as { error: string; message: string };
      expect(explainFailure(new ApiError(r!.status, body.error, body.message)).missing).toBe(true);
    }
    expect(await m.handle(new Request("http://m/v1/other"), new URL("http://m/v1/other"))).toBeNull();
  });
});

describe("generated matches", () => {
  const wire = {
    players: [{ slot: 0, team: 0, lives: 0, unit: 4096 }, { slot: 1, team: 1, lives: 0, unit: 4097 }],
    labelled: [{ label: "hill_corner", object: 256, number: 0, placement: null }, { label: "ctf_flag_return", object: 257, number: 1, placement: null, team: 1 }],
    teams: [0, 1],
    variant: { team_play: 1, score_limit: 50, time_limit: 300 },
    options: null, seed: 9, round_time: 9000, ticks: 400, start: true,
    timeline: [
      { tick: 12, events: [{ type: "enter", zone: 256, object: 4097 }, { type: "pickup", player: 0, object: 16384 }] },
      { tick: 40, events: [{ type: "kill", victim: 1, killer: 0 }, { type: "fly" }, { type: "actor_kill", killer: null, class: "grunt" }] },
    ],
  };
  const summary = {
    asked: 25, ran: 24,
    wins: [{ winner: { kind: "team", index: 0 }, matches: 14 }, { winner: { kind: "team", index: 1 }, matches: 9 }, { winner: { kind: "draw" }, matches: 1 }],
    ends: [{ end: "score_limit", matches: 22 }, { end: "time_limit", matches: 2 }],
    avg_score: 49.5, max_score: 50, avg_ticks: 5700, max_ticks: 9000,
    worst_tick_ops: 112, worst_tick_seed: 14, budget: 20000,
    errors: [{ seed: 17, message: "script error at tick 4, in on kill, op 2: a table row is out of range" }],
    flags: [{ kind: "nobody_scores", text: "nobody scored in 3 of 24 matches", seed: 11, matches: 3 }, { kind: "never_ran", text: "on spawn never ran", seed: 9, matches: 24 }],
    stopped: "24 of 25 matches ran: a batch starts no match past 24000000 ops or 400000 ticks",
  };
  const answer = {
    ok: true, teams: 2, summary,
    rows: [
      { seed: 9, end: "score_limit", winner: { kind: "team", index: 0 }, ticks: 400, top_score: 50, ops: 900, worst_tick_ops: 14 },
      { seed: 10, end: "time_limit", winner: { kind: "draw" }, ticks: 9000, top_score: 31, ops: 800, worst_tick_ops: 14 },
    ],
    match: {
      seed: 9, end: "score_limit", why: null, winner: { kind: "team", index: 0 }, top_score: 50, scenario: wire,
      trace: { ok: true, hash: "ab".repeat(32), ticks: [], last_tick: 400, error: null, round_ended: null, limit: "score", cut: null, ops: 900, worst_tick_ops: 14, scores: [{ slot: 0, score: 50 }, { slot: 1, score: 3 }] },
    },
  };

  test("a setup stays in range, and the die never lands on the same seed", () => {
    expect(defaultSetup()).toEqual({ players: 8, teamPlay: false, seed: 1 });
    expect(parseSetup({ players: 40, teamPlay: true, seed: -3 })).toEqual({ players: 16, teamPlay: true, seed: 0 });
    expect(parseSetup({ players: 1, teamPlay: "yes", seed: 2.5 })).toEqual({ players: 2, teamPlay: false, seed: 1 });
    expect(parseSetup(null)).toEqual(defaultSetup());
    expect(rerolled(5, () => 0)).toBe(1);
    expect(rerolled(1, () => 0)).toBe(2);
    expect(rerolled(MAX_SEED, () => 0.9999999)).toBe(1);
    for (let i = 0; i < 50; i++) { const s = rerolled(7); expect(s).not.toBe(7); expect(s >= 1 && s <= MAX_SEED).toBe(true); }
  });

  test("the request names the players, the teams, the seed and how many", () => {
    expect(simRequest(SRC, { players: 6, teamPlay: true, seed: 4 }, SIM_MATCHES)).toEqual({ source: SRC, players: 6, teams: 2, seed: 4, matches: 25 });
    expect(simRequest(SRC, { players: 6, teamPlay: false, seed: 4 }, 1, 11)).toEqual({ source: SRC, players: 6, teams: 0, seed: 11, matches: 1 });
  });

  test("a match's scenario reads into a draft and builds back into a run", () => {
    const d = draftOfWire(wire);
    expect(d.players).toEqual(wire.players);
    expect(d.teamPlay).toBe(true);
    expect(d.seed).toBe(9);
    expect(d.ticks).toBe(400);
    expect(d.roundTime).toBe(9000);
    expect(d.variant).toEqual(wire.variant);
    expect(d.objects).toEqual([{ id: 256, label: "hill_corner", number: 0, team: null }, { id: 257, label: "ctf_flag_return", number: 1, team: 1 }]);
    // An event type the page does not know is left out; the rest keep their order.
    expect(d.events).toEqual([
      { tick: 12, type: "enter", zone: 256, object: 4097 },
      { tick: 12, type: "pickup", player: 0, object: 16384 },
      { tick: 40, type: "kill", victim: 1, killer: 0 },
      { tick: 40, type: "actor_kill", killer: null, class: "grunt" },
    ]);
    const built = buildScenario(d);
    expect(built.variant).toEqual(wire.variant);
    expect(built.players).toEqual(wire.players);
    expect(built.labelled).toEqual([{ label: "hill_corner", object: 256, number: 0 }, { label: "ctf_flag_return", object: 257, number: 1, team: 1 }]);
    // A hand-seated player has no unit, and none is sent.
    expect(buildScenario(defaultScenario()).players).toEqual([{ slot: 0, team: 0, lives: 0 }, { slot: 1, team: 1, lives: 0 }]);
    expect(built.timeline).toEqual([
      { tick: 12, events: wire.timeline[0]!.events },
      { tick: 40, events: [{ type: "kill", victim: 1, killer: 0 }, { type: "actor_kill", killer: null, class: "grunt" }] },
    ]);
    // Turning team play off in the editor wins over the record's field.
    expect((buildScenario({ ...d, teamPlay: false }).variant as Record<string, number>).team_play).toBe(0);
    expect(scenarioProblems(d)).toEqual([]);
    expect(draftOfWire(null)).toEqual({ players: [], events: [], ticks: 0, roundTime: 0, seed: 0, teamPlay: false });
  });

  test("objects read by what they are", () => {
    const objects = draftOfWire(wire).objects!;
    expect(objectName(UNIT_BASE + 3)).toBe("Player 3");
    expect(objectName(256, objects)).toBe("hill_corner 0");
    expect(objectName(257, objects)).toBe("ctf_flag_return (team 1)");
    expect(objectName(SPAWNED_BASE, objects)).toBe("objective 1");
    expect(objectName(77, objects)).toBe("object 77");
    expect(describeEvent({ tick: 0, type: "enter", zone: 256, object: UNIT_BASE + 1 }, objects)).toBe("Player 1 entered hill_corner 0");
    expect(describeEvent({ tick: 0, type: "pickup", player: 0, object: SPAWNED_BASE }, objects)).toBe("Player 0 picked up objective 1");
    expect(describeEvent({ tick: 0, type: "bounce", player: 2, object: 513 })).toBe("Player 2 bounced on object 513");
    expect(describeEvent({ tick: 0, type: "actor_kill", killer: 2, class: "grunt" })).toBe("Player 2 killed a grunt");
    expect(describeEvent({ tick: 0, type: "squad_dead", squad: 3 })).toBe("Squad 3 died");
    expect(wireEvent({ tick: 1, type: "actor_assist", killer: 1, class: "elite" })).toEqual({ type: "actor_assist", killer: 1, assist: null, class: "elite" });
    expect(newEvent("squad_dead", 9, [])).toEqual({ tick: 9, type: "squad_dead", squad: 0 });
  });

  test("an answer reads back with its summary, rows and match", () => {
    const r = parseSim(answer) as SimResult;
    expect(simFailed(r)).toBe(false);
    expect(r.teams).toBe(2);
    expect(r.summary!.wins[0]).toEqual({ winner: { kind: "team", index: 0 }, matches: 14 });
    expect(r.summary!.wins[2]!.winner).toEqual({ kind: "draw", index: null });
    expect(r.summary!.ends).toEqual([{ end: "score_limit", matches: 22 }, { end: "time_limit", matches: 2 }]);
    expect(r.rows.map((x) => x.seed)).toEqual([9, 10]);
    expect(r.match.seed).toBe(9);
    expect(r.match.trace.limit).toBe("score");
    expect(r.match.scenario.events.length).toBe(4);
    // The existing results read a generated match unchanged.
    expect(standings(r.match.trace, r.match.scenario).map((s) => [s.slot, s.team, s.score])).toEqual([[0, 0, 50], [1, 1, 3]]);
    expect(timeline(r.match.trace, r.match.scenario)[0]!.happened).toEqual(["Player 1 entered hill_corner 0", "Player 0 picked up objective 1"]);
    expect(traceLines(r.match.trace)).toEqual(["the game ended at tick 400 on the score limit"]);

    const f = parseSim({ ok: false, errors: [{ line: 2, col: 1, message: "nope" }] });
    expect(simFailed(f)).toBe(true);
    const junk = parseSim({ ok: true, summary: 7, rows: "x", match: { end: "exploded", winner: 3 } }) as SimResult;
    expect(junk.summary).toBeNull();
    expect(junk.rows).toEqual([]);
    expect(junk.match.end).toBe("cap");
    expect(junk.match.winner).toEqual({ kind: "nobody", index: null });
    expect(junk.match.trace.ticks).toEqual([]);
  });

  test("winners, endings and lengths read as words", () => {
    const team = (n: number) => `${["Red", "Blue"][n]} team`;
    expect(winnerName({ kind: "player", index: 3 })).toBe("Player 3");
    expect(winnerName({ kind: "team", index: 1 }, team)).toBe("Blue team");
    expect(winnerName({ kind: "draw", index: null })).toBe("Draw");
    expect(winnerName({ kind: "nobody", index: null })).toBe("Nobody");
    expect(endLabel("score_limit")).toBe("Score limit");
    expect(endLabel("cap")).toBe("Never ended");
    expect(ofMatches(9, 25)).toBe("9 of 25");
    expect(shortTime(5700)).toBe("3:10");
    expect(shortTime(0)).toBe("0:00");
  });

  test("warnings list errors first, each with the seed that shows it", () => {
    const r = parseSim(answer) as SimResult;
    expect(simWarnings(r.summary!)).toEqual([
      { text: "Script error at tick 4, in on kill, op 2: a table row is out of range", seed: 17, bad: true },
      { text: "Nobody scored in 3 of 24 matches", seed: 11, bad: false },
      { text: "On spawn never ran", seed: 9, bad: false },
      { text: "24 of 25 matches ran: a batch starts no match past 24000000 ops or 400000 ticks", seed: null, bad: false },
    ]);
    expect(simWarnings({ ...r.summary!, errors: [], flags: [], stopped: null })).toEqual([]);
    expect(simWarnings({ ...r.summary!, errors: [], stopped: null, flags: [{ kind: "never_ends", text: "the game never ended in every match (20 minutes passed)", seed: 2, matches: 5 }] })[0]!.bad).toBe(true);
  });

  test("a match's neighbours are the seeds beside it", () => {
    const rows = (parseSim(answer) as SimResult).rows;
    expect(neighbourSeed(rows, 9, 1)).toBe(10);
    expect(neighbourSeed(rows, 9, -1)).toBeNull();
    expect(neighbourSeed(rows, 10, 1)).toBeNull();
    expect(neighbourSeed(rows, 99, 1)).toBeNull();
  });

  test("simulate posts the setup and passes its signal on", async () => {
    const { f, calls } = recorder(() => json(answer));
    const stop = new AbortController();
    const r = await simulateRules("http://x/v1", SRC, { players: 4, teamPlay: true, seed: 9 }, 25, undefined, f, stop.signal);
    expect(simFailed(r)).toBe(false);
    expect(calls[0]!.url).toBe("http://x/v1/rules/simulate");
    expect(JSON.parse(String(calls[0]!.init.body))).toEqual({ source: SRC, players: 4, teams: 2, seed: 9, matches: 25 });
    expect(calls[0]!.init.signal).toBe(stop.signal);
    await simulateRules("http://x/v1", SRC, { players: 4, teamPlay: false, seed: 9 }, 1, 12, f);
    expect(JSON.parse(String(calls[1]!.init.body))).toEqual({ source: SRC, players: 4, teams: 0, seed: 12, matches: 1 });
  });
});

describe("the mock's generated matches", async () => {
  const { MockRules, mockSimulate } = await import("../mock/rules");
  const { World } = await import("../mock/world");
  const call = async (body: unknown) => {
    const m = new MockRules(new World());
    return m.handle(new Request("http://m/v1/rules/simulate", { method: "POST", body: JSON.stringify(body) }), new URL("http://m/v1/rules/simulate"));
  };

  test("the same seed gives the same batch, and another seed another", async () => {
    const a = await call({ source: SRC, players: 4, seed: 3, matches: 5 });
    const b = await call({ source: SRC, players: 4, seed: 3, matches: 5 });
    expect(a!.status).toBe(200);
    expect(a!.body).toEqual(b!.body);
    const c = await call({ source: SRC, players: 4, seed: 4, matches: 5 });
    expect(JSON.stringify(c!.body)).not.toBe(JSON.stringify(a!.body));
  });

  test("a batch sums up its matches in the server's shape", async () => {
    const r = parseSim((await call({ source: SRC, players: 6, teams: 2, seed: 3, matches: 8 }))!.body) as SimResult;
    expect(simFailed(r)).toBe(false);
    expect(r.teams).toBe(2);
    expect(r.rows.map((x) => x.seed)).toEqual([3, 4, 5, 6, 7, 8, 9, 10]);
    const s = r.summary!;
    expect([s.asked, s.ran, s.budget]).toEqual([8, 8, 20000]);
    expect(s.wins.reduce((n, w) => n + w.matches, 0)).toBe(8);
    expect(s.ends.reduce((n, e) => n + e.matches, 0)).toBe(8);
    expect(s.max_score).toBe(Math.max(...r.rows.map((x) => x.top_score)));
    expect(r.match.seed).toBe(3);
    expect(r.match.scenario.players.length).toBe(6);
    expect(r.match.scenario.teamPlay).toBe(true);
    expect(r.match.scenario.events.every((e) => e.type === "kill")).toBe(true);
    expect(r.rows[0]!.winner).toEqual(r.match.winner);
    // A match that reached the limit ends at its last kill.
    if (r.match.end === "score_limit") expect(r.match.trace.last_tick).toBe(r.match.scenario.events.at(-1)!.tick);
    // The page's standings and timeline read it.
    expect(standings(r.match.trace, r.match.scenario).length).toBe(6);
    expect(timeline(r.match.trace, r.match.scenario).length).toBe(r.match.scenario.events.length);
  });

  test("one match has no summary, and its scenario runs again to the same scores", async () => {
    const m = new MockRules(new World());
    const one = parseSim((await call({ source: SRC, seed: 21 }))!.body) as SimResult;
    expect(one.summary).toBeNull();
    expect(one.rows).toEqual([]);
    expect(one.match.scenario.players.length).toBe(8);
    const again = await m.handle(new Request("http://m/v1/rules/run", { method: "POST", body: JSON.stringify({ source: SRC, scenario: buildScenario(one.match.scenario) }) }), new URL("http://m/v1/rules/run"));
    expect((parseTrace(again!.body) as Trace).scores).toEqual(one.match.trace.scores);
  });

  test("a script that scores nothing is flagged, a broken one reads as its errors, a bad setup is refused", async () => {
    const idle = mockSimulate('rules "Idle" 1\ncarrier slayer\non tick {\n}\n', { players: 4, matches: 3 }) as unknown as { summary: { flags: Array<{ kind: string; seed: number }> } };
    expect(idle.summary.flags.map((f) => f.kind)).toEqual(["nobody_scores", "clock"]);
    expect(idle.summary.flags[0]!.seed).toBe(1);
    const broken = parseSim((await call({ source: "carrier slayer" }))!.body);
    expect(simFailed(broken)).toBe(true);
    for (const bad of [{ players: 0 }, { players: 17 }, { teams: 9 }, { matches: 0 }, { matches: 101 }, { seed: "x" }]) {
      expect((await call({ source: SRC, ...bad }))!.status).toBe(400);
    }
    expect((await call({ players: 4 }))!.status).toBe(400);
  });
});
