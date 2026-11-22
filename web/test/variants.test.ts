// Game variants on Live (docs/live/live-api.md "Game variants"): the name
// and description rules, the file name, option values coerced to what the
// script declares, answers read defensively, the calls with their token
// scope, the Blob download, the check's options and carrier, and the
// mock's routes. Every gamertag and variant is invented.

import { describe, expect, test } from "bun:test";
import {
  VARIANT_DESCRIPTION_MAX, VARIANT_NAME_MAX, bundleHref, bundleText, coerceOption, coerceOptions, defaultOptions, deleteVariant,
  descriptionProblem, downloadBundle, fetchVariants, isBundle, makeBundle, nameProblem, parseBundleAnswer, parseVariant,
  parseVariants, publishFromScript, publishVariant, scriptHref, variantFileName, variantHref, type H2Variant,
} from "../src/lib/variants";
import { CONTENT_WRITE, carrierOf, parseCheck, parseOptions, type ScriptOption } from "../src/lib/rules";
import type { LocalAccount } from "../src/state/account";
import { parseJws } from "../src/state/verify";

const RFC = { d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo", kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k" };
const account = (xuid = "0009000000000042", gamertag = "afr"): LocalAccount => ({ xuid, gamertag, id: `acct:${xuid}@localhost`, server: "http://127.0.0.1:8074", server_id: "localhost", kid: RFC.kid, key: RFC.d, x: RFC.x });

function recorder(reply: (url: string, init: RequestInit) => Response) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = (async (url: string, init: RequestInit = {}) => { calls.push({ url, init }); return reply(url, init); }) as unknown as typeof fetch;
  return { f, calls };
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const headerOf = (init: RequestInit, name: string): string | undefined => (init.headers as Record<string, string> | undefined)?.[name];
const lxmOf = (init: RequestInit): string => (parseJws(headerOf(init, "authorization")!.slice(7))!.payload as { lxm: string }).lxm;

const ID = "cd".repeat(32);
const HASH = "ab".repeat(32);
const RECORD = "00".repeat(0x130);
const VIEW = {
  id: ID, name: "Pillow Fight", description: "Swords only.", author: { xuid: "0009000000000042", gamertag: "afr" },
  engine: 2, game_type: "Slayer", record_hex: RECORD, script_hash: HASH, script_name: "Slayer", visibility: "unlisted",
  created: "2026-11-20T10:00:00Z", bundle_url: `/v1/variants/${ID}/h2variant`, settings: [{ label: "Score to win", value: "25" }],
  eligible: ["custom", "hopper"],
};
const BUNDLE: H2Variant = { format: "h2recomp-game-variant", version: 2, name: "Pillow Fight", description: "", game_type: "Slayer", engine: 2, record_hex: RECORD };

const INT: ScriptOption = { name: "kill_points", label: "Kill points", description: null, kind: "int", lo: 1, hi: 5, default: 1, values: [] };
const BOOL: ScriptOption = { name: "suicide_costs", label: "Suicide penalty", description: null, kind: "bool", lo: 0, hi: 1, default: true, values: [] };
const ENUM: ScriptOption = { name: "mode", label: "Mode", description: null, kind: "enum", lo: 0, hi: 2, default: "fast", values: ["slow", "fast", "wild"] };

describe("names and descriptions", () => {
  test("a good name passes", () => {
    expect(nameProblem("Pillow Fight")).toBeNull();
    expect(nameProblem("x".repeat(VARIANT_NAME_MAX))).toBeNull();
  });

  test("each rule refuses", () => {
    expect(nameProblem("")).not.toBeNull();
    expect(nameProblem("x".repeat(VARIANT_NAME_MAX + 1))).toContain("31");
    expect(nameProblem(" Fight")).toContain("space");
    expect(nameProblem("Fight ")).toContain("space");
    expect(nameProblem("Pillow\tFight")).toContain("control");
    expect(nameProblem("Pillow|Fight")).toContain("|");
    expect(nameProblem("Pillow \u{1F600}")).toContain("U+FFFF");
    expect(nameProblem("A \ue405 B")).toContain("glyph");
    expect(nameProblem("A \ue407 B")).toContain("glyph");
    expect(nameProblem("A \ue404 B")).toBeNull();
  });

  test("descriptions", () => {
    expect(descriptionProblem("")).toBeNull();
    expect(descriptionProblem("x".repeat(VARIANT_DESCRIPTION_MAX))).toBeNull();
    expect(descriptionProblem("x".repeat(VARIANT_DESCRIPTION_MAX + 1))).toContain("150");
    expect(descriptionProblem("two\nlines")).toContain("control");
    expect(descriptionProblem("a|b")).toContain("|");
  });

  test("file names replace what file systems refuse", () => {
    expect(variantFileName("Pillow Fight")).toBe("Pillow Fight.h2variant");
    expect(variantFileName('a/b\\c:d*e?f"g<h>i|j')).toBe("a_b_c_d_e_f_g_h_i_j.h2variant");
    expect(variantFileName("trailing dots...")).toBe("trailing dots.h2variant");
    expect(variantFileName("..")).toBe("game variant.h2variant");
    expect(variantFileName("")).toBe("game variant.h2variant");
  });
});

describe("options", () => {
  test("an int is whole and in range", () => {
    expect(coerceOption(INT, "3")).toBe(3);
    expect(coerceOption(INT, 9)).toBe(5);
    expect(coerceOption(INT, -2)).toBe(1);
    expect(coerceOption(INT, 2.6)).toBe(3);
    expect(coerceOption(INT, "")).toBe(1);
    expect(coerceOption(INT, "x")).toBe(1);
  });

  test("a bool and an enum", () => {
    expect(coerceOption(BOOL, false)).toBe(false);
    expect(coerceOption(BOOL, "false")).toBe(false);
    expect(coerceOption(BOOL, undefined)).toBe(true);
    expect(coerceOption(ENUM, "wild")).toBe("wild");
    expect(coerceOption(ENUM, 0)).toBe("slow");
    expect(coerceOption(ENUM, "nope")).toBe("fast");
  });

  test("every declared option, and only those", () => {
    expect(coerceOptions([INT, BOOL, ENUM], { kill_points: "4", other: 1 })).toEqual({ kill_points: 4, suicide_costs: true, mode: "fast" });
    expect(defaultOptions([INT, BOOL])).toEqual({ kill_points: 1, suicide_costs: true });
    expect(coerceOptions([], { kill_points: 2 })).toEqual({});
  });

  test("the check's options and carrier", () => {
    const c = parseCheck({
      ok: true, hash: HASH, name: "Test", version: 1, errors: [], report: null,
      options: [
        { name: "kill_points", label: "Kill points", description: "Points for each kill", kind: "int", lo: 1, hi: 5, default: 1 },
        { name: "mode", label: "Mode", kind: "enum", lo: 0, hi: 2, default: "fast", values: ["slow", "fast", "wild"] },
        { name: "odd", kind: "float" },
      ],
    });
    expect(c.options.map((o) => o.name)).toEqual(["kill_points", "mode"]);
    expect(c.options[0]!.description).toBe("Points for each kill");
    expect(c.options[1]!.default).toBe("fast");
    expect(c.carrier).toBeNull();
    expect(parseCheck({ ok: true, carrier: "king" }).carrier!.engine).toBe(4);
    expect(parseOptions(undefined)).toEqual([]);
    expect(carrierOf('rules "X" 1\ncarrier territories\n')!.label).toBe("Territories");
    expect(carrierOf('rules "X" 1\n')).toBeNull();
  });
});

describe("reading answers", () => {
  test("a variant", () => {
    const v = parseVariant(VIEW)!;
    expect(v.visibility).toBe("unlisted");
    expect(v.script_hash).toBe(HASH);
    expect(v.settings).toEqual([{ label: "Score to win", value: "25" }]);
    expect(v.eligible).toEqual(["custom", "hopper"]);
    expect(parseVariant({ id: ID })!.eligible).toEqual([]);
    expect(parseVariant({ ...VIEW, id: "nope" })).toBeNull();
    expect(parseVariant({ id: ID })).toMatchObject({ name: "Untitled", script_hash: null, visibility: "public", bundle_url: `/v1/variants/${ID}/h2variant` });
    expect(parseVariants({ items: [VIEW, { id: 3 }, null] }).length).toBe(1);
    expect(parseVariants(null)).toEqual([]);
  });

  test("a bundle answer", () => {
    expect(isBundle(BUNDLE)).toBe(true);
    expect(isBundle({ ...BUNDLE, version: 1 })).toBe(false);
    expect(parseBundleAnswer({ id: ID, bundle: BUNDLE }).file_name).toBe("Pillow Fight.h2variant");
    expect(() => parseBundleAnswer({ id: ID, bundle: {} })).toThrow("unreadable");
  });

  test("links", () => {
    expect(variantHref(ID)).toBe(`/games/halo2/variants/${ID}`);
    expect(scriptHref(HASH)).toBe(`/games/halo2/rules?script=${HASH}`);
    expect(bundleHref("http://live.test", VIEW.bundle_url)).toBe(`http://live.test/v1/variants/${ID}/h2variant`);
    expect(bundleHref("http://live.test", "https://cdn.test/x")).toBe("https://cdn.test/x");
  });
});

describe("the calls", () => {
  test("a list sends the reader's token when there is one", async () => {
    const { f, calls } = recorder(() => json({ items: [VIEW] }));
    expect((await fetchVariants("http://x/v1", "Sneaky Pete", null, f))[0]!.id).toBe(ID);
    expect(calls[0]!.url).toBe("http://x/v1/variants?author=Sneaky%20Pete");
    expect(headerOf(calls[0]!.init, "authorization")).toBeUndefined();
    await fetchVariants("http://x/v1", "afr", "tok", f);
    expect(headerOf(calls[1]!.init, "authorization")).toBe("Bearer tok");
  });

  test("publishing and deleting sign content.write; a bundle signs nothing", async () => {
    const { f, calls } = recorder((url, init) => {
      if (url.endsWith("/bundle")) return json({ id: ID, file_name: "x.h2variant", bundle: BUNDLE });
      if (init.method === "DELETE") return json({ ok: true, id: ID });
      return json(VIEW, 201);
    });
    const made = await makeBundle("http://x/v1", { source: "src" }, { name: "Pillow Fight", description: "", options: { kill_points: 2 } }, f);
    expect(made.bundle.name).toBe("Pillow Fight");
    expect(JSON.parse(String(calls[0]!.init.body))).toEqual({ source: "src", name: "Pillow Fight", description: "", options: { kill_points: 2 } });
    expect(headerOf(calls[0]!.init, "authorization")).toBeUndefined();

    expect((await publishVariant("http://x/v1", "localhost", account(), BUNDLE, "public", f)).id).toBe(ID);
    expect(calls[1]!.url).toBe("http://x/v1/variants");
    expect(JSON.parse(String(calls[1]!.init.body))).toEqual({ bundle: BUNDLE, visibility: "public" });
    expect(lxmOf(calls[1]!.init)).toBe(CONTENT_WRITE);

    await publishFromScript("http://x/v1", "localhost", account(), { script_hash: HASH, name: "P", description: "", visibility: "unlisted", options: {} }, f);
    expect(calls[2]!.url).toBe("http://x/v1/variants/from-script");
    expect(lxmOf(calls[2]!.init)).toBe(CONTENT_WRITE);

    await deleteVariant("http://x/v1", "localhost", account(), ID, f);
    expect(calls[3]!.url).toBe(`http://x/v1/variants/${ID}`);
    expect(calls[3]!.init.method).toBe("DELETE");
    expect(lxmOf(calls[3]!.init)).toBe(CONTENT_WRITE);
  });

  test("a refusal is an ApiError with the server's message", async () => {
    const { f } = recorder(() => json({ error: "invalid", message: "a name is at most 31 characters" }, 400));
    await expect(publishVariant("http://x/v1", "localhost", account(), BUNDLE, "public", f)).rejects.toThrow("at most 31");
  });

  test("download goes through a Blob link", () => {
    const clicked: Array<{ href: string; download: string }> = [];
    const doc = { createElement: () => { const a = { href: "", download: "", click() { clicked.push({ href: a.href, download: a.download }); } }; return a; } } as unknown as Pick<Document, "createElement">;
    downloadBundle(BUNDLE, "Pillow Fight.h2variant", doc);
    expect(clicked.length).toBe(1);
    expect(clicked[0]!.download).toBe("Pillow Fight.h2variant");
    expect(clicked[0]!.href.startsWith("blob:")).toBe(true);
    expect(JSON.parse(bundleText(BUNDLE))).toEqual(BUNDLE);
  });
});

describe("the mock's variants", async () => {
  const { MockVariants } = await import("../mock/variants");
  const { MockRules } = await import("../mock/rules");
  const { World } = await import("../mock/world");
  const { issueToken } = await import("../src/admin/token");

  const setup = () => {
    const w = new World();
    w.seed(10_000);
    const rules = new MockRules(w);
    return { w, m: new MockVariants(w, rules) };
  };
  const call = (m: InstanceType<typeof MockVariants>, method: string, path: string, body?: unknown, token?: string) =>
    m.handle(new Request(`http://m${path}`, { method, headers: token ? { authorization: `Bearer ${token}` } : {}, ...(body === undefined ? {} : { body: JSON.stringify(body) }) }), new URL(`http://m${path}`));

  test("seeded variants list for their author; others pass", async () => {
    const { m } = setup();
    const r = await call(m, "GET", `/v1/variants?author=${encodeURIComponent("Sneaky Pete")}`);
    const items = parseVariants(r!.body);
    expect(items.length).toBe(2);
    expect(items.some((v) => v.script_name === "Slayer")).toBe(true);
    const one = await call(m, "GET", `/v1/variants/${items[0]!.id}`);
    expect(parseVariant(one!.body)!.name).toBe(items[0]!.name);
    const file = await call(m, "GET", `/v1/variants/${items[0]!.id}/h2variant`);
    expect(isBundle(file!.body)).toBe(true);
    expect(file!.headers!["content-disposition"]).toContain(".h2variant");
    expect(await m.handle(new Request("http://m/v1/rules"), new URL("http://m/v1/rules"))).toBeNull();
  });

  test("bundle, publish, unlisted for the author only, delete", async () => {
    const { w, m } = setup();
    const pete = w.accountByTag("Sneaky Pete")!;
    const me = account(pete.xuid, pete.gamertag);
    const src = 'rules "Mine" 1\ncarrier king\non kill(v, k) {\n    score(k, 1)\n}\n';
    const b = await call(m, "POST", "/v1/variants/bundle", { source: src, name: "Hill Fight", description: "", options: {} });
    const made = parseBundleAnswer(b!.body);
    expect(made.bundle.engine).toBe(4);
    expect((await call(m, "POST", "/v1/variants", { bundle: made.bundle, visibility: "unlisted" }))!.status).toBe(401);
    const write = () => issueToken(me, "localhost", CONTENT_WRITE);
    const pub = await call(m, "POST", "/v1/variants", { bundle: made.bundle, visibility: "unlisted" }, await write());
    expect(pub!.status).toBe(201);
    const again = await call(m, "POST", "/v1/variants", { bundle: made.bundle, visibility: "unlisted" }, await write());
    expect(again!.status).toBe(200);
    const id = parseVariant(pub!.body)!.id;
    const anon = parseVariants((await call(m, "GET", `/v1/variants?author=${pete.xuid}`))!.body);
    expect(anon.some((v) => v.id === id)).toBe(false);
    const read = parseVariants((await call(m, "GET", `/v1/variants?author=${pete.xuid}`, undefined, await issueToken(me, "localhost", "read")))!.body);
    expect(read.some((v) => v.id === id)).toBe(true);
    const bad = await call(m, "POST", "/v1/variants/from-script", { script_hash: HASH, name: "X", description: "", visibility: "public", options: {} }, await write());
    expect(bad!.status).toBe(404);
    const del = await call(m, "DELETE", `/v1/variants/${id}`, undefined, await write());
    expect(del!.body).toEqual({ ok: true, id });
    expect((await call(m, "GET", `/v1/variants/${id}`))!.status).toBe(404);
  });

  test("a bad name is refused", async () => {
    const { m } = setup();
    const r = await call(m, "POST", "/v1/variants/bundle", { source: 'rules "X" 1\ncarrier slayer\n', name: " bad", description: "", options: {} });
    expect(r!.status).toBe(400);
  });
});
