// The workbench editor's pure half (src/lib/rulesIde.ts; docs/game/
// rules-engine.md "Workbench"): position and range conversion, check
// errors as markers, the semantic token encoding, completion and symbol
// kinds, the route's answers read defensively, the request client, the
// Insert menus, and the mock's IDE route. Nothing here loads Monaco.

import { describe, expect, test } from "bun:test";
import {
  API_ENTRIES, CAP_WORDS, CONTROL, EVENTS, FLAG_LABELS, IdeClient, KEYWORDS, LABEL_ITEMS, LSP_COMPLETION_KINDS, MENU_PROBES, MONARCH,
  TIME_LITERAL, TYPES, apiGroups, apiSnippet, applyEdits, byteColToUtf16, completionKindName, encodeTokens, eventSnippet,
  flagLabelItems, fromLspRange, ideBody, insertMenus, labelDeclaration, labelName, mapLabelItems, mergeApi, mergeEvents, offsetAt,
  parseActions, parseCompletions, parseFormat, parseHover, parseLegend, parseLocations, parseRename, parseSignature, parseSymbols,
  parseTokens, problemMarker, symbolKindName, toLsp, type IdeOp,
} from "../src/lib/rulesIde";

const NO_DELAY: Record<IdeOp, number> = { complete: 0, hover: 0, signature: 0, definition: 0, references: 0, tokens: 0, symbols: 0, format: 0, actions: 0, rename: 0 };
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

  test("the probes ask at a blank top-level line, a blank statement and a new label's name", () => {
    for (const p of MENU_PROBES) {
      const line = p.source.split("\n")[p.line]!;
      expect(line.trim()).toBe(p.which === "label" ? "label" : "");
      expect(line.length).toBe(p.character);
    }
  });

  test("the Declarations menu has its Labels group, and an In the test map group for undeclared labels", () => {
    const plain = insertMenus(null, null)[0]!;
    expect(plain.groups.map((g) => g.title)).toEqual(["Declarations", "Labels"]);
    expect(plain.groups[0]!.items.map((i) => i.label)).toContain("const");
    const labels = plain.groups[1]!.items;
    expect(labels.slice(0, LABEL_ITEMS.length).map((i) => i.label)).toEqual(["one zone", "one object", "a set of objects", "a numbered set of zones", "spawn areas"]);
    expect(labels.find((i) => i.label === "a numbered set of zones")!.snippet).toBe("label ${1:capture}: zones numbered ${2:0}..${3:1}");
    // The built-in flag labels, whole: the page's list until the route gives its own.
    expect(labels.slice(LABEL_ITEMS.length).map((i) => i.snippet)).toEqual(FLAG_LABELS.map(([t, d]) => `label ${t}: ${d}`));
    const route = flagLabelItems([
      { label: "label territory: zones 1..8", kind: 15, detail: "a netgame flag's label", documentation: "The label every map gives...\nmore", insertText: "territory: zones 1..8", snippet: false },
      { label: "if", kind: 14, detail: "", documentation: "", insertText: "if", snippet: false },
    ]);
    expect(route).toEqual([{ label: "label territory: zones 1..8", detail: "label territory: zones 1..8", words: "The label every map gives...", snippet: "label territory: zones 1..8" }]);
    expect(flagLabelItems([])).toHaveLength(FLAG_LABELS.length);

    // The test map's labels the script does not declare, each as a declaration.
    const things = [
      { label: "goal", number: null, class: "volume" },
      { label: "capture", number: 0, class: "flag" },
      { label: "capture", number: 1, class: "volume" },
      { label: "barrier", number: null, class: "object" },
      { label: "barrier", number: null },
      { label: "attack", number: null, class: "spawn" },
      { label: "red base", number: null, class: "object" },
      { label: "if", number: null },
    ];
    const items = mapLabelItems(things, ["goal"]);
    expect(items.map((i) => i.snippet)).toEqual([
      "label capture: zones numbered",
      "label barrier: objects",
      "label attack: spawns",
      'label red_base: object = "red base"',
      'label if_label: object = "if"',
    ]);
    expect(items[0]!.words).toBe("2 things in the test map");
    const menus = insertMenus(null, null, null, items);
    expect(menus[0]!.groups.map((g) => g.title)).toEqual(["Declarations", "Labels", "In the test map"]);
    expect(labelName("3rd zone")).toBe("_3rd_zone");
    expect(labelDeclaration("hill", [{ number: 0, class: "flag" }])).toBe("label hill: zone");
  });

  test("the Control menu has match", () => {
    const control = insertMenus(null, null)[3]!.groups[0]!.items;
    expect(control.find((i) => i.label === "match")!.snippet.startsWith("match ${1:value} {")).toBe(true);
  });
});

describe("the language's words", () => {
  test("time literals, const, match and carrier", () => {
    expect(KEYWORDS as readonly string[]).toContain("const");
    expect(KEYWORDS as readonly string[]).toContain("carrier");
    expect(CONTROL as readonly string[]).toContain("match");
    for (const t of ["time", "zone", "weapon", "carrier"]) expect(TYPES as readonly string[]).toContain(t);
    expect(MONARCH.types).toContain("zones");
    for (const lit of ["30s", "1.5s", "300t"]) expect(TIME_LITERAL.exec(lit)?.[0]).toBe(lit);
    expect(TIME_LITERAL.exec("30")).toBeNull();
    expect(TIME_LITERAL.exec("3sec")).toBeNull();
    // The grammar reads a time before a plain number, and `carrier.x` and `const x` by place.
    const rules = MONARCH.tokenizer.root.map((r) => String(r[0]));
    expect(rules.indexOf(String(TIME_LITERAL))).toBeLessThan(rules.indexOf(String(/\d+\.\d+/)));
    expect(rules.some((r) => r.includes("carrier"))).toBe(true);
    expect(rules.some((r) => r.includes("const"))).toBe(true);
  });
});

describe("quick fixes and rename", () => {
  const SRC = 'rules "T" 1\ncarrier slayer\nlabel barrier: objects\n\non tick {\n    for each barier b { delete(b) }\n}\n';
  const at = (line: number, a: number, b: number) => ({ start: { line, character: a }, end: { line, character: b } });

  test("a code action applies its edits", () => {
    const actions = parseActions([
      { title: "Change to `barrier`", kind: "quickfix", diagnostic: { range: at(5, 13, 19), severity: 1, message: "no label named barier. Did you mean barrier?", code: "label.unknown", source: "rules" }, edits: [{ range: at(5, 13, 19), newText: "barrier" }] },
      { title: "Declare label `barier`", kind: "quickfix", diagnostic: { range: at(5, 13, 19), code: "label.unknown" }, edits: [{ range: at(3, 0, 0), newText: "label barier: objects\n" }] },
      { title: "no edits", kind: "quickfix", diagnostic: {}, edits: [] },
      { kind: "quickfix" },
    ]);
    expect(actions.map((a) => a.title)).toEqual(["Change to `barrier`", "Declare label `barier`"]);
    expect(actions[0]!.code).toBe("label.unknown");
    expect(actions[0]!.range).toEqual(at(5, 13, 19));
    expect(applyEdits(SRC, actions[0]!.edits)).toBe(SRC.replace("barier b", "barrier b"));
    expect(applyEdits(SRC, actions[1]!.edits)).toBe(SRC.replace("\n\non tick", "\nlabel barier: objects\n\non tick"));
    // Several edits apply against the text before any of them.
    const both = applyEdits("let a = a + a", [{ range: at(0, 4, 5), newText: "bb" }, { range: at(0, 8, 9), newText: "bb" }, { range: at(0, 12, 13), newText: "bb" }]);
    expect(both).toBe("let bb = bb + bb");
    expect(offsetAt("ab\ncd", { line: 1, character: 9 })).toBe(5);
    expect(offsetAt("ab\ncd", { line: 7, character: 0 })).toBe(5);
    expect(applyEdits("x", [])).toBe("x");
  });

  test("a rename's edits, and the reason a refused one gives", async () => {
    expect(parseRename({ edits: [{ range: at(2, 6, 13), newText: "wall" }, { range: null, newText: "x" }] })).toEqual({ edits: [{ range: at(2, 6, 13), newText: "wall" }] });
    expect(parseRename({ error: "if is a keyword" })).toEqual({ error: "if is a keyword" });
    expect(parseRename(null)).toBeNull();
    expect(ideBody("actions", "s", { line: 1, character: 2 }, { end: { line: 1, character: 5 } })).toEqual({ source: "s", op: "actions", line: 1, character: 2, end_line: 1, end_character: 5 });
    expect(ideBody("rename", "s", { line: 1, character: 2 }, { newName: "wall" })).toEqual({ source: "s", op: "rename", line: 1, character: 2, new_name: "wall" });

    const calls: Array<Record<string, unknown>> = [];
    const refused = new IdeClient(() => "/v1", (async (_url: string, init: RequestInit = {}) => {
      calls.push(JSON.parse(String(init.body)));
      return json({ error: "invalid", message: "if is a keyword" }, 400);
    }) as unknown as typeof fetch, NO_DELAY);
    expect(await refused.request("rename", SRC, 1, { line: 2, character: 8 }, undefined, { newName: "if" })).toEqual({ error: "if is a keyword" });
    expect(calls[0]).toMatchObject({ op: "rename", new_name: "if" });
    // A rename is never kept: asking again asks the server again.
    await refused.request("rename", SRC, 1, { line: 2, character: 8 }, undefined, { newName: "if" });
    expect(calls).toHaveLength(2);
    // An action at another range is another answer.
    const ok = new IdeClient(() => "/v1", (async (_url: string, init: RequestInit = {}) => { calls.push(JSON.parse(String(init.body))); return json([]); }) as unknown as typeof fetch, NO_DELAY);
    await ok.request("actions", SRC, 1, { line: 5, character: 13 }, undefined, { end: { line: 5, character: 19 } });
    await ok.request("actions", SRC, 1, { line: 5, character: 13 }, undefined, { end: { line: 5, character: 19 } });
    await ok.request("actions", SRC, 1, { line: 5, character: 13 }, undefined, { end: { line: 5, character: 14 } });
    expect(calls).toHaveLength(4);
    expect(calls[2]).toMatchObject({ op: "actions", end_line: 5, end_character: 19 });
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
    const bad = await r.handle(post({ source: SRC, op: "fly", line: 0, character: 0 }), u("/v1/rules/ide"));
    expect(bad!.status).toBe(400);
  });

  test("quick fixes and rename", async () => {
    const { mockRename } = await import("../mock/rules");
    const src = 'rules "T" 1\ncarrier slayer\nlabel barrier: objects\nlabel core: object\n\non tick {\n    for each barier b { delete(b) }\n}\n';
    const fixes = parseActions(mockIde(src, "actions", 6, 13));
    expect(fixes.map((a) => a.title)).toEqual(["Change to `barrier`"]);
    expect(applyEdits(src, fixes[0]!.edits)).toContain("for each barrier b");
    const unused = parseActions(mockIde(src, "actions", 3, 6));
    expect(unused.map((a) => [a.title, a.code])).toEqual([["Remove the declaration", "label.unused"]]);
    expect(applyEdits(src, unused[0]!.edits)).not.toContain("label core");

    const fixed = applyEdits(src, fixes[0]!.edits);
    const renamed = mockRename(fixed, 2, 8, "wall");
    if (typeof renamed === "string") throw new Error(renamed);
    // The label keeps its map text.
    expect(applyEdits(fixed, renamed.edits)).toContain('label wall: objects = "barrier"');
    expect(applyEdits(fixed, renamed.edits)).toContain("for each wall b");
    expect(mockRename(fixed, 2, 8, "if")).toBe("if is a keyword");
    expect(mockRename(fixed, 2, 8, "core")).toBe("core is declared already");
    const r = new MockRules(new World());
    const post = (body: unknown) => new Request("http://mock/v1/rules/ide", { method: "POST", body: JSON.stringify(body) });
    const u = new URL("http://mock/v1/rules/ide");
    expect((await r.handle(post({ source: fixed, op: "rename", line: 2, character: 8, new_name: "wall" }), u))!.status).toBe(200);
    const no = await r.handle(post({ source: fixed, op: "rename", line: 2, character: 8, new_name: "1x" }), u);
    expect(no!.status).toBe(400);
    expect((no!.body as { message: string }).message).toBe("1x is not a name");
  });
});
