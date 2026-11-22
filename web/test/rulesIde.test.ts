// The workbench editor's pure half (src/lib/rulesIde.ts; docs/game/
// rules-engine.md "Workbench"): position and range conversion, check
// errors as markers, the semantic token encoding, completion and symbol
// kinds, the route's answers read defensively, the request client, the
// Insert menus, and the mock's IDE route. Nothing here loads Monaco.

import { describe, expect, test } from "bun:test";
import {
  API_ENTRIES, CAP_WORDS, EVENTS, IdeClient, LSP_COMPLETION_KINDS, MENU_PROBES, apiGroups, apiSnippet, byteColToUtf16,
  completionKindName, encodeTokens, eventSnippet, fromLspRange, insertMenus, mergeApi, mergeEvents, parseCompletions,
  parseFormat, parseHover, parseLegend, parseLocations, parseSignature, parseSymbols, parseTokens, problemMarker,
  symbolKindName, toLsp, type IdeOp,
} from "../src/lib/rulesIde";

const NO_DELAY: Record<IdeOp, number> = { complete: 0, hover: 0, signature: 0, definition: 0, references: 0, tokens: 0, symbols: 0, format: 0 };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

describe("positions", () => {
  test("Monaco's 1-based line and column are the route's 0-based line and character", () => {
    expect(toLsp(1, 1)).toEqual({ line: 0, character: 0 });
    expect(toLsp(12, 5)).toEqual({ line: 11, character: 4 });
    expect(toLsp(0, 0)).toEqual({ line: 0, character: 0 });
  });

  test("an LSP range is a Monaco range one up on both ends", () => {
    expect(fromLspRange({ start: { line: 0, character: 0 }, end: { line: 2, character: 7 } }))
      .toEqual({ startLineNumber: 1, startColumn: 1, endLineNumber: 3, endColumn: 8 });
  });

  test("a check's byte column counts UTF-16 units on the editor's side", () => {
    expect(byteColToUtf16("score(p, 1)", 7)).toBe(7);
    // "é" is two bytes and one unit; "😀" four bytes and two units.
    expect(byteColToUtf16("\"é\" x", 6)).toBe(5);
    expect(byteColToUtf16("\"😀\" x", 8)).toBe(6);
    // Past the line's end: its end.
    expect(byteColToUtf16("ab", 40)).toBe(3);
    expect(byteColToUtf16("", 1)).toBe(1);
  });

  test("an error marks the word at its place, or one character", () => {
    const src = "rules \"T\" 1\non kill(v, k) {\n    scor(k, 1)\n}\n";
    expect(problemMarker(src, { line: 3, col: 5, message: "no entry scor" }))
      .toEqual({ startLineNumber: 3, startColumn: 5, endLineNumber: 3, endColumn: 9, message: "no entry scor" });
    expect(problemMarker(src, { line: 2, col: 15, message: "x" })).toMatchObject({ startColumn: 15, endColumn: 16 });
    expect(problemMarker(src, { line: 0, col: 0, message: "budget" })).toBeNull();
    // A line past the end marks the last one.
    expect(problemMarker("a\nb", { line: 9, col: 1, message: "x" })).toMatchObject({ startLineNumber: 2 });
  });
});

describe("semantic tokens", () => {
  const legend = ["keyword", "function", "variable"];

  test("encode as line and start deltas in document order", () => {
    const data = encodeTokens([
      { line: 2, start: 4, length: 5, type: "function" },
      { line: 0, start: 0, length: 5, type: "keyword" },
      { line: 2, start: 10, length: 1, type: "variable" },
      { line: 0, start: 8, length: 3, type: "keyword" },
    ], legend);
    expect(data).toEqual([
      0, 0, 5, 0, 0,
      0, 8, 3, 0, 0,
      2, 4, 5, 1, 0,
      0, 6, 1, 2, 0,
    ]);
  });

  test("types by index follow the answer's own legend, and unknown ones drop", () => {
    expect(encodeTokens([{ line: 0, start: 0, length: 2, type: 0 }], legend, ["variable"])).toEqual([0, 0, 2, 2, 0]);
    expect(encodeTokens([{ line: 0, start: 0, length: 2, type: 1 }], legend)).toEqual([0, 0, 2, 1, 0]);
    expect(encodeTokens([{ line: 0, start: 0, length: 2, type: "macro" }, { line: 0, start: 3, length: 0, type: "keyword" }], legend)).toEqual([]);
  });

  test("the route's tokens and legend are read defensively", () => {
    expect(parseTokens({ legend: ["keyword"], data: [{ line: 1, start: 2, length: 3, type: "keyword" }, { line: -1, start: 0, length: 1, type: 0 }, { line: 0 }] }))
      .toEqual({ legend: ["keyword"], data: [{ line: 1, start: 2, length: 3, type: "keyword" }] });
    expect(parseTokens(null)).toBeNull();
    expect(parseLegend(["a", "b"])).toEqual(["a", "b"]);
    expect(parseLegend({ legend: ["a"] })).toEqual(["a"]);
    expect(parseLegend({ tokenTypes: ["t"] })).toEqual(["t"]);
    expect(parseLegend({})).toBeNull();
  });
});

describe("kinds", () => {
  test("LSP completion kinds map to Monaco's names", () => {
    expect(completionKindName(3)).toBe("Function");
    expect(completionKindName(14)).toBe("Keyword");
    expect(completionKindName(15)).toBe("Snippet");
    expect(completionKindName(23)).toBe("Event");
    expect(completionKindName(25)).toBe("TypeParameter");
    expect(completionKindName(0)).toBe("Text");
    expect(completionKindName(99)).toBe("Text");
    expect(LSP_COMPLETION_KINDS.length).toBe(25);
  });

  test("LSP symbol kinds map to Monaco's names", () => {
    expect(symbolKindName(12)).toBe("Function");
    expect(symbolKindName(13)).toBe("Variable");
    expect(symbolKindName(24)).toBe("Event");
    expect(symbolKindName(0)).toBe("Variable");
  });
});

describe("the route's answers", () => {
  test("completions, with snippet format and markdown documentation", () => {
    const items = parseCompletions([
      { label: "score", kind: 3, detail: "score(p, n)", documentation: { kind: "markdown", value: "core" }, insertText: "score(${1:p}, ${2:n})", insertTextFormat: 2 },
      { label: "if", kind: 14 },
      { kind: 3 },
    ]);
    expect(items).toEqual([
      { label: "score", kind: 3, detail: "score(p, n)", documentation: "core", insertText: "score(${1:p}, ${2:n})", snippet: true },
      { label: "if", kind: 14, detail: "", documentation: "", insertText: "if", snippet: false },
    ]);
    expect(parseCompletions({ isIncomplete: false, items: [{ label: "x" }] })).toHaveLength(1);
  });

  test("hover, signature help, locations, symbols and format", () => {
    expect(parseHover({ contents: { kind: "markdown", value: "**x**" }, range: { start: { line: 0, character: 1 }, end: { line: 0, character: 2 } } }))
      .toEqual({ contents: "**x**", range: { start: { line: 0, character: 1 }, end: { line: 0, character: 2 } } });
    expect(parseHover({ contents: "" })).toBeNull();
    expect(parseHover(null)).toBeNull();

    const sig = parseSignature({ signatures: [{ label: "score(p, n)", documentation: "core", parameters: [{ label: "p" }, { label: [9, 10] }] }], activeSignature: 4, activeParameter: 1 });
    expect(sig).toEqual({ signatures: [{ label: "score(p, n)", documentation: "core", parameters: ["p", "n"] }], activeSignature: 0, activeParameter: 1 });
    expect(parseSignature({ signatures: [] })).toBeNull();

    const r = { start: { line: 1, character: 0 }, end: { line: 1, character: 3 } };
    expect(parseLocations([{ range: r }, { range: null }])).toEqual([r]);
    expect(parseLocations({ range: r })).toEqual([r]);
    expect(parseLocations(null)).toEqual([]);

    const syms = parseSymbols([{ name: "on kill", kind: 24, range: r, children: [{ name: "x", kind: 13, range: r }] }, { name: "bad" }]);
    expect(syms).toHaveLength(1);
    expect(syms[0]!.selectionRange).toEqual(r);
    expect(syms[0]!.children[0]!.name).toBe("x");

    expect(parseFormat({ text: "a\n" })).toEqual({ text: "a\n" });
    expect(parseFormat({ error: "line 3: unexpected }" })).toEqual({ error: "line 3: unexpected }" });
    expect(parseFormat({})).toBeNull();
  });
});

describe("the request client", () => {
  function recorder() {
    const calls: Array<{ url: string; body: Record<string, unknown> }> = [];
    const f = (async (url: string, init: RequestInit = {}) => {
      calls.push({ url, body: JSON.parse(String(init.body)) });
      return json([{ label: "score", kind: 3 }]);
    }) as unknown as typeof fetch;
    return { f, calls };
  }

  test("sends the contract's body and caches per version and position", async () => {
    const { f, calls } = recorder();
    const c = new IdeClient(() => "/v1", f, NO_DELAY);
    const a = await c.request("complete", "src", 1, { line: 2, character: 4 });
    expect(a).toEqual([{ label: "score", kind: 3 }]);
    expect(calls[0]).toEqual({ url: "/v1/rules/ide", body: { source: "src", op: "complete", line: 2, character: 4 } });
    await c.request("complete", "src", 1, { line: 2, character: 4 });
    expect(calls).toHaveLength(1);
    await c.request("complete", "src", 1, { line: 2, character: 5 });
    expect(calls).toHaveLength(2);
    await c.request("complete", "src2", 2, { line: 2, character: 4 });
    expect(calls).toHaveLength(3);
    // Document ops are one answer a version.
    await c.request("tokens", "src2", 2, { line: 0, character: 0 });
    await c.request("tokens", "src2", 2, { line: 5, character: 1 });
    expect(calls).toHaveLength(4);
  });

  test("a newer request of an op abandons the older one", async () => {
    const { f, calls } = recorder();
    const c = new IdeClient(() => "/v1", f, { ...NO_DELAY, hover: 20 });
    const first = c.request("hover", "a", 1, { line: 0, character: 0 });
    const second = c.request("hover", "a", 1, { line: 0, character: 1 });
    expect(await first).toBeNull();
    expect(await second).toEqual([{ label: "score", kind: 3 }]);
    expect(calls).toHaveLength(1);
  });

  test("a cancelled, refused or failed request answers null", async () => {
    const { f } = recorder();
    const c = new IdeClient(() => "/v1", f, { ...NO_DELAY, hover: 20 });
    const ctl = new AbortController();
    const p = c.request("hover", "a", 1, { line: 0, character: 0 }, ctl.signal);
    ctl.abort();
    expect(await p).toBeNull();
    const refused = new IdeClient(() => "/v1", (async () => json({ error: "not_found" }, 404)) as unknown as typeof fetch, NO_DELAY);
    expect(await refused.request("hover", "a", 1, { line: 0, character: 0 })).toBeNull();
    const broken = new IdeClient(() => "/v1", (async () => { throw new Error("offline"); }) as unknown as typeof fetch, NO_DELAY);
    expect(await broken.request("hover", "a", 1, { line: 0, character: 0 })).toBeNull();
  });
});

describe("the Insert menus", () => {
  test("every event and API entry has a snippet that parses as one", () => {
    for (const e of EVENTS) {
      const s = eventSnippet(e);
      expect(s.startsWith(`on ${e.name}`)).toBe(true);
      expect(s).toContain("$0");
    }
    expect(eventSnippet({ name: "kill", params: ["victim", "killer"], words: "" })).toBe("on kill(${1:victim}, ${2:killer}) {\n\t$0\n}");
    expect(eventSnippet({ name: "tick", params: [], words: "" })).toBe("on tick {\n\t$0\n}");
    const score = API_ENTRIES.find((e) => e.name === "score")!;
    expect(apiSnippet(score)).toBe("score(${1:p}, ${2:n})");
    for (const e of API_ENTRIES) {
      const s = apiSnippet(e);
      expect(s.startsWith(e.name + "(")).toBe(true);
      // Balanced placeholders.
      expect((s.match(/\$\{/g) ?? []).length).toBe((s.match(/\}/g) ?? []).length);
    }
  });

  test("the API groups by capability, in plain words", () => {
    const groups = apiGroups(API_ENTRIES);
    const titles = groups.map((g) => g.title);
    expect(titles[0]).toBe("Reads");
    expect(titles[1]).toBe("Core actions");
    expect(titles.at(-1)).toBe("Built-ins");
    const team = groups.find((g) => g.title === "player.team")!;
    expect(team.words).toBe(CAP_WORDS["player.team"]!);
    expect(team.items.map((i) => i.label)).toEqual(["set_team"]);
    expect(team.items[0]!.words).toBe("needs requires player.team");
    expect(groups.find((g) => g.title === "Core actions")!.items.map((i) => i.label)).toContain("score");
  });

  test("the four menus, static without the route", () => {
    const menus = insertMenus(null, null);
    expect(menus.map((m) => m.id)).toEqual(["declarations", "events", "api", "control"]);
    const decl = menus[0]!.groups[0]!.items.map((i) => i.label);
    for (const w of ["option", "global var", "player var", "team var", "object var", "enum", "table", "palette", "fn"]) expect(decl).toContain(w);
    expect(menus[1]!.groups[0]!.items.map((i) => i.label)).toContain("on kill(victim, killer)");
    const control = menus[3]!.groups[0]!.items.map((i) => i.label);
    for (const w of ["if", "if / else", "for each player", "let"]) expect(control).toContain(w);
  });

  test("the route's completions merge over the static lists", () => {
    const merged = mergeApi([
      { label: "score", kind: 3, detail: "score(p: player, n: int)", documentation: "", insertText: "score(${1:who}, ${2:1})", snippet: true },
      { label: "new_thing", kind: 3, detail: "new_thing()", documentation: "does a thing\nmore", insertText: "new_thing()", snippet: false },
      { label: "if", kind: 14, detail: "", documentation: "", insertText: "if", snippet: false },
    ]);
    expect(merged.entries.find((e) => e.name === "score")!.snippet).toBe("score(${1:who}, ${2:1})");
    expect(merged.extra).toEqual([{ label: "new_thing", detail: "new_thing()", words: "does a thing", snippet: "new_thing($0)" }]);

    const events = mergeEvents([{ label: "on kill(victim, killer)", kind: 23, detail: "", documentation: "a kill", insertText: "on kill(${1:v}, ${2:k}) {\n\t$0\n}", snippet: true }]);
    const kill = events.find((e) => e.label === "on kill(victim, killer)")!;
    expect(kill.snippet).toBe("on kill(${1:v}, ${2:k}) {\n\t$0\n}");
    expect(kill.words).toBe("a kill");
    expect(events).toHaveLength(EVENTS.length);
  });

  test("the probes ask at a blank top-level line and a blank statement", () => {
    for (const p of MENU_PROBES) {
      const line = p.source.split("\n")[p.line]!;
      expect(line.trim()).toBe("");
      expect(line.length).toBe(p.character);
    }
  });
});

describe("the mock's IDE route", async () => {
  const { MockRules, mockIde, MOCK_LEGEND } = await import("../mock/rules");
  const { World } = await import("../mock/world");
  const SRC = 'rules "T" 1\ncarrier slayer\nplayer var hits: int = 0\n\non kill(victim, killer) {\n    score(killer, 1)\n    killer.hits = 1\n}\n';

  test("completes calls in a statement and handlers at the top level", () => {
    const body = parseCompletions(mockIde(SRC, "complete", 5, 4));
    expect(body.some((c) => c.label === "score" && c.snippet)).toBe(true);
    const top = parseCompletions(mockIde(SRC, "complete", 3, 0));
    expect(top.some((c) => c.label === "on kill(victim, killer)")).toBe(true);
  });

  test("hovers, signatures, definitions, references, tokens, symbols, format", () => {
    expect(parseHover(mockIde(SRC, "hover", 5, 6))!.contents).toContain("score(p, n)");
    expect(parseSignature(mockIde(SRC, "signature", 5, 18))!.activeParameter).toBe(1);
    expect(parseLocations(mockIde(SRC, "definition", 6, 13))).toEqual([{ start: { line: 2, character: 11 }, end: { line: 2, character: 15 } }]);
    expect(parseLocations(mockIde(SRC, "references", 5, 12)).length).toBe(3);
    const t = parseTokens(mockIde(SRC, "tokens", 0, 0))!;
    expect(t.legend).toEqual(MOCK_LEGEND);
    expect(t.data.some((d) => d.type === "event" && d.line === 4)).toBe(true);
    expect(parseSymbols(mockIde(SRC, "symbols", 0, 0)).map((s) => s.name)).toEqual(["hits", "on kill"]);
    expect(parseFormat(mockIde("rules \"T\" 1\non tick {\nscore(none, 1)   \n}", "format", 0, 0))).toEqual({ text: "rules \"T\" 1\non tick {\n    score(none, 1)\n}\n" });
    expect(parseFormat(mockIde("rules \"T\" 1\non tick {\n", "format", 0, 0))).toMatchObject({ error: expect.any(String) });
  });

  test("the routes", async () => {
    const r = new MockRules(new World());
    const u = (p: string) => new URL(`http://mock${p}`);
    const legend = await r.handle(new Request("http://mock/v1/rules/ide/legend"), u("/v1/rules/ide/legend"));
    expect(legend!.body).toEqual({ legend: MOCK_LEGEND });
    const post = (body: unknown) => new Request("http://mock/v1/rules/ide", { method: "POST", body: JSON.stringify(body) });
    const ok = await r.handle(post({ source: SRC, op: "hover", line: 5, character: 6 }), u("/v1/rules/ide"));
    expect(ok!.status).toBe(200);
    const bad = await r.handle(post({ source: SRC, op: "rename", line: 0, character: 0 }), u("/v1/rules/ide"));
    expect(bad!.status).toBe(400);
  });
});
