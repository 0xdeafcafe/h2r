// The mod repository (/games/halo2/mods): the new MapSummary fields, the list
// query and its URL state, the cursor stack, labels, and the mock's list
// (sorts, filters and cursors as the server's API says). Every name, id and
// size here is invented.

import { describe, expect, test } from "bun:test";
import { parseMapSummaries, parseMapSummary, safeSourceUrl } from "../src/lib/maps";
import {
  MOD_CATEGORIES, baseOptions, categoryLabel, creditBeyondAuthor, cursorCurrent, cursorNext, cursorPage, cursorPrev, cursorStart,
  downloadsText, isModCategory, modHref, modListParams, modMissingText, modQueryFromSearch, modQueryKey, modSearchParams, modsHref, shortSha,
} from "../src/lib/mods";

const ID = "a1".repeat(32);
const wire = (over: Record<string, unknown> = {}) => ({
  map_id: ID,
  name: "Teacup Crates",
  author: { xuid: "0009000000000001", gamertag: "Sneaky Pete" },
  base: { map: "teacup", derived: false },
  layers: [{ sha256: "d4".repeat(32), size: 2_000, kind: "patch" }],
  size: 2_000,
  published_at: "2026-11-16T10:00:00Z",
  ...over,
});

describe("parsing the new fields", () => {
  test("position, downloads, credit, category and source_url are read", () => {
    const m = parseMapSummary(wire({ position: 42, downloads: 7, credit: "Old Crate Co.", category: "Weapons", source_url: "https://mods.example.net/crates" }))!;
    expect(m.position).toBe(42);
    expect(m.downloads).toBe(7);
    expect(m.credit).toBe("Old Crate Co.");
    expect(m.category).toBe("weapons");
    expect(m.source_url).toBe("https://mods.example.net/crates");
  });

  test("absent ones default: position and downloads 0, the rest left out", () => {
    const m = parseMapSummary(wire())!;
    expect(m.position).toBe(0);
    expect(m.downloads).toBe(0);
    expect("credit" in m || "category" in m || "source_url" in m).toBe(false);
    expect(parseMapSummary(wire({ downloads: -3, position: "9" }))!).toMatchObject({ downloads: 0, position: 0 });
  });

  test("a source_url that isn't http or https is dropped", () => {
    for (const bad of ["javascript:alert(1)", "data:text/html,hi", "ftp://mods.example.net/x", "file:///etc/passwd", "not a url", " ", 7]) {
      expect(parseMapSummary(wire({ source_url: bad }))!.source_url).toBeUndefined();
      expect(safeSourceUrl(bad)).toBeNull();
    }
    expect(safeSourceUrl("http://mods.example.net/a b")).toBe("http://mods.example.net/a%20b");
    expect(parseMapSummaries([wire({ source_url: "JavaScript:alert(1)" })])[0]!.source_url).toBeUndefined();
  });
});

describe("the list query and the page URL", () => {
  test("reads the URL leniently", () => {
    expect(modQueryFromSearch({})).toEqual({ sort: "new" });
    expect(modQueryFromSearch({ sort: "popular", base: "Teacup", category: "VISUALS", q: "  rain ", author: "Sneaky Pete" }))
      .toEqual({ sort: "popular", base: "teacup", category: "visuals", q: "rain", author: "Sneaky Pete" });
    expect(modQueryFromSearch({ sort: "oldest", category: "hats", q: "" })).toEqual({ sort: "new" });
    expect(modQueryFromSearch(new URLSearchParams("sort=popular&base=orchard"))).toEqual({ sort: "popular", base: "orchard" });
    expect(modQueryFromSearch({ base: ["orchard", "teacup"] })).toEqual({ sort: "new", base: "orchard" });
  });

  test("writes it back, the default sort left out, and round-trips", () => {
    expect(modSearchParams({ sort: "new" })).toEqual({ sort: undefined, base: undefined, category: undefined, author: undefined, q: undefined });
    expect(modsHref({ sort: "new" })).toBe("/games/halo2/mods");
    const q = { sort: "popular" as const, base: "teacup", category: "maps" as const, q: "crate box" };
    const href = modsHref(q);
    expect(href).toBe("/games/halo2/mods?sort=popular&base=teacup&category=maps&q=crate+box");
    expect(modQueryFromSearch(new URL(href, "http://x").searchParams)).toEqual(q);
  });

  test("the API query carries the filters, the limit and the cursor", () => {
    expect(modListParams({ sort: "new" }, null)).toEqual({ sort: "new", base: undefined, category: undefined, author: undefined, q: undefined, limit: 20, cursor: undefined });
    expect(modListParams({ sort: "popular", base: "teacup", category: "weapons", q: "x" }, "7:42", 5))
      .toEqual({ sort: "popular", base: "teacup", category: "weapons", author: undefined, q: "x", limit: 5, cursor: "7:42" });
  });

  test("the query key changes with any filter, not with nothing", () => {
    expect(modQueryKey({ sort: "new" })).toBe(modQueryKey(modQueryFromSearch({})));
    expect(modQueryKey({ sort: "new" })).not.toBe(modQueryKey({ sort: "popular" }));
    expect(modQueryKey({ sort: "new", q: "a" })).not.toBe(modQueryKey({ sort: "new", base: "a" }));
  });
});

describe("the cursor stack", () => {
  test("forward on next, back to where it was, never past the first page", () => {
    let c = cursorStart();
    expect([cursorCurrent(c), cursorPage(c)]).toEqual([null, 0]);
    expect(cursorPrev(c)).toBe(c);
    c = cursorNext(c, "42");
    c = cursorNext(c, "30");
    expect([cursorCurrent(c), cursorPage(c)]).toEqual(["30", 2]);
    expect(cursorNext(c, null)).toBe(c);
    c = cursorPrev(c);
    expect([cursorCurrent(c), cursorPage(c)]).toEqual(["42", 1]);
    c = cursorPrev(c);
    expect([cursorCurrent(c), cursorPage(c)]).toEqual([null, 0]);
  });
});

describe("labels and bits", () => {
  test("every category has a label, unknown ones read as uncategorized", () => {
    expect(MOD_CATEGORIES).toEqual(["gameplay", "weapons", "vehicles", "visuals", "maps", "other"]);
    expect(MOD_CATEGORIES.map(categoryLabel)).toEqual(["Gameplay", "Weapons", "Vehicles", "Visuals", "Map Changes", "Other"]);
    expect(categoryLabel(undefined)).toBe("Uncategorized");
    expect(categoryLabel("hats")).toBe("Uncategorized");
    expect(isModCategory("maps")).toBe(true);
    expect(isModCategory("Maps")).toBe(false);
  });

  test("base map options: seen ones and the chosen one, unique, by title", () => {
    const title = (m: string) => ({ teacup: "Teacup", orchard: "Orchard", lighthouse: "Lighthouse" })[m] ?? m;
    expect(baseOptions(["teacup", "Orchard", "teacup"], "lighthouse", title).map((b) => b.map)).toEqual(["lighthouse", "orchard", "teacup"]);
    expect(baseOptions([], undefined, title)).toEqual([]);
  });

  test("credit only when it names someone else", () => {
    const author = { xuid: "1", gamertag: "Sneaky Pete" };
    expect(creditBeyondAuthor({ author, credit: "Old Crate Co." })).toBe("Old Crate Co.");
    expect(creditBeyondAuthor({ author, credit: "sneaky pete" })).toBeNull();
    expect(creditBeyondAuthor({ author })).toBeNull();
  });

  test("counts, hashes, links and missing states", () => {
    expect(downloadsText(1)).toBe("1 download");
    expect(downloadsText(1204)).toBe("1,204 downloads");
    expect(shortSha("ab".repeat(32))).toBe("abababababab");
    expect(modHref(ID)).toBe(`/games/halo2/mods/${ID}`);
    expect(modMissingText("unknown").text).toMatch(/no mod/i);
    expect(modMissingText("gone").text).toMatch(/taken down by an admin/);
  });
});

describe("the mock's list", async () => {
  const { Content } = await import("../mock/content");
  const { Admin } = await import("../mock/admin");
  const { World } = await import("../mock/world");
  const world = new World();
  const content = new Content(world, new Admin(world, false));
  const get = async (path: string) => {
    const r = await content.handle(new Request("http://m" + path), new URL("http://m" + path), () => new Response(null));
    return r instanceof Response ? { status: r.status, body: null as unknown } : r!;
  };
  const list = async (qs: string) => {
    const r = await get("/v1/maps?" + qs);
    const b = r.body as { items: unknown[]; next: string | null; position: number };
    return { status: r.status, items: parseMapSummaries(b?.items), next: b?.next ?? null };
  };
  const all = async (qs: string, limit: number) => {
    const out: string[] = [];
    let cursor: string | null = null;
    for (let i = 0; i < 20; i++) {
      const p = await list(`${qs}&limit=${limit}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`);
      out.push(...p.items.map((m) => m.name));
      if (!p.next) break;
      cursor = p.next;
    }
    return out;
  };

  test("newest first by position; pages chain through `next` with nothing lost or repeated", async () => {
    const first = await list("sort=new");
    const positions = first.items.map((m) => m.position);
    expect(positions).toEqual([...positions].sort((a, b) => b - a));
    expect(first.items.length).toBe(10);
    expect(first.next).toBeNull();
    const page = await list("sort=new&limit=3");
    expect(page.next).toBe(String(page.items[2]!.position));
    expect(await all("sort=new", 3)).toEqual(first.items.map((m) => m.name));
  });

  test("most downloaded first, ties newest first, with a downloads:position cursor", async () => {
    const p = await list("sort=popular");
    const keys = p.items.map((m) => [m.downloads, m.position] as const);
    for (let i = 1; i < keys.length; i++) {
      const [d0, p0] = keys[i - 1]!, [d1, p1] = keys[i]!;
      expect(d0 > d1 || (d0 === d1 && p0 > p1)).toBe(true);
    }
    const page = await list("sort=popular&limit=2");
    expect(page.next).toBe(`${page.items[1]!.downloads}:${page.items[1]!.position}`);
    expect(await all("sort=popular", 2)).toEqual(p.items.map((m) => m.name));
  });

  test("filters by base, category, author and text; bad sort or cursor is a 400", async () => {
    expect((await list("base=TEACUP")).items.every((m) => m.base.map === "teacup")).toBe(true);
    expect((await list("category=visuals")).items.map((m) => m.category)).toEqual(["visuals", "visuals"]);
    expect((await list("q=FOG")).items.map((m) => m.name)).toEqual(["Lighthouse Fog Bank"]);
    expect((await list("q=old crate")).items.map((m) => m.name)).toEqual(["Teacup Crates"]); // the credit
    expect((await list("q=lighthouse")).items.every((m) => m.base.map === "lighthouse")).toBe(true);
    const tag = (await list("")).items[0]!.author.gamertag;
    expect((await list("author=" + encodeURIComponent(tag.toUpperCase()))).items.every((m) => m.author.gamertag === tag)).toBe(true);
    expect((await list("sort=oldest")).status).toBe(400);
    expect((await list("sort=popular&cursor=12")).status).toBe(400);
  });

  test("an unsafe source_url never reaches the page; downloads and position are on the package", async () => {
    const towers = (await list("q=orchard towers")).items[0]!;
    expect(towers.source_url).toBeUndefined();
    const r = await get(`/v1/maps/${towers.map_id}`);
    const map = parseMapSummary((r.body as { map: unknown }).map)!;
    expect(map.position).toBe(towers.position);
    expect(map.downloads).toBe(towers.downloads);
  });
});
