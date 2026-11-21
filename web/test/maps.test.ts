// Custom and modded maps on Live (docs/game/maps.md §5): reading packages, the
// per-page lookup cache, and how sessions and games that name a map_id are
// shown. Every id, name and size here is invented.

import { describe, expect, test } from "bun:test";
import {
  PackageCache, readModPhoto, formatBytes, geometryChoice, isMapId, lookupFromResponse, mapGroupKey, mapGroupLabel,
  packImageUrl, packageImageUrl, packageScreenshotUrl, packageTitle, parseMapSummaries, parseMapSummary, pictureChoice, type PackageLookup,
} from "../src/lib/maps";
import { parseSession } from "../src/lib/sessions";
import { applyEvents, applyGameSnapshot, initialState } from "../src/state/reducer";
import { campaignRecord } from "../src/state/derive";
import { ev, resetPos } from "./helpers";

const ID = "a1".repeat(32);
const ID2 = "b2".repeat(32);

const wire = (over: Record<string, unknown> = {}) => ({
  map_id: ID,
  name: "Lockdown Remix",
  description: "More crates.",
  author: { xuid: "0009000000000001", gamertag: "Sneaky Pete" },
  base: { map: "teacup", sha256: "c3".repeat(32), derived: false },
  layers: [{ sha256: "d4".repeat(32), size: 2_000_000, kind: "patch" }, { sha256: "e5".repeat(32), size: 516_576, kind: "pack" }],
  size: 2_516_576,
  published_at: "2026-11-16T10:00:00Z",
  ...over,
});
const ok = (over?: Record<string, unknown>): PackageLookup => lookupFromResponse(200, { map: wire(over), missing: [], position: 7 });

describe("full-size photos", () => {
  test("only raster bytes get a dashboard blob URL, with the detected type", async () => {
    const mislabeled = new Response(new Uint8Array([0xff, 0xd8, 0xff, 0xe0]), {
      headers: { "content-type": "text/plain" },
    });
    expect((await readModPhoto(mislabeled)).type).toBe("image/jpeg");
    await expect(readModPhoto(new Response("<svg/>", { headers: { "content-type": "image/png" } }))).rejects.toThrow("expected");
    await expect(readModPhoto(new Response(new Uint8Array(512 * 1024 + 1)))).rejects.toThrow("too large");
  });
});

describe("reading packages", () => {
  test("a summary keeps what the server sent and leaves absent options out", () => {
    const m = parseMapSummary(wire())!;
    expect(m).toMatchObject({ map_id: ID, name: "Lockdown Remix", author: { gamertag: "Sneaky Pete" }, base: { map: "teacup", derived: false }, size: 2_516_576 });
    expect(m.layers.map((l) => l.kind)).toEqual(["patch", "pack"]);
    expect("image" in m).toBe(false);
    expect("geometry" in m).toBe(false);
    expect(parseMapSummary(wire({ image: "f6".repeat(32), geometry: "07".repeat(32) }))).toMatchObject({ image: "f6".repeat(32), geometry: "07".repeat(32) });
  });

  test("no valid id, name or base map: dropped; size falls back to the layers' sum", () => {
    expect(parseMapSummary(wire({ map_id: "ABC" }))).toBeNull();
    expect(parseMapSummary(wire({ map_id: ID.toUpperCase() }))).toBeNull();
    expect(parseMapSummary(wire({ name: " " }))).toBeNull();
    expect(parseMapSummary(wire({ base: {} }))).toBeNull();
    expect(parseMapSummary(wire({ size: undefined }))!.size).toBe(2_516_576);
    expect(parseMapSummaries([wire(), { nope: 1 }, wire({ map_id: ID2 })]).map((m) => m.map_id)).toEqual([ID, ID2]);
    expect(parseMapSummaries(null)).toEqual([]);
  });

  test("map ids are 64 lowercase hex", () => {
    expect(isMapId(ID)).toBe(true);
    expect(isMapId(ID.slice(1))).toBe(false);
    expect(isMapId("teacup")).toBe(false);
  });

  test("detail responses: 200 is the package, 404 unknown, 410 gone, others an error", () => {
    expect(ok()).toMatchObject({ state: "ok", map: { name: "Lockdown Remix" }, missing: [] });
    expect(lookupFromResponse(200, { map: wire(), missing: ["d4".repeat(32), 3] })).toMatchObject({ missing: ["d4".repeat(32)] });
    expect(lookupFromResponse(404, { error: "not_found" })).toEqual({ state: "unknown" });
    expect(lookupFromResponse(410, { error: "gone", message: "blocked" })).toEqual({ state: "gone" });
    expect(lookupFromResponse(500, null)).toEqual({ state: "error" });
    expect(lookupFromResponse(200, { map: { junk: true } })).toEqual({ state: "error" });
  });
});

describe("sizes", () => {
  test("bytes, KB and MB like the download line wants them", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(812)).toBe("812 B");
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(24 * 1024 + 300)).toBe("24 KB");
    expect(formatBytes(2_516_576)).toBe("2.4 MB");
    expect(formatBytes(32 * 1024 * 1024)).toBe("32 MB");
    expect(formatBytes(1.5 * 1024 ** 3)).toBe("1.5 GB");
  });
});

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("the lookup cache", () => {
  test("asks once per id, and keeps 404 and 410 as answers", async () => {
    const asked: string[] = [];
    let changes = 0;
    const replies: Record<string, { status: number; body?: unknown }> = { [ID]: { status: 410, body: { error: "gone" } }, [ID2]: { status: 404 } };
    const c = new PackageCache(async (id) => { asked.push(id); return replies[id]!; }, () => changes++);
    expect(c.get(ID)).toEqual({ state: "loading" });
    expect(c.get(ID)).toEqual({ state: "loading" });
    c.get(ID2);
    await tick();
    expect(c.get(ID)).toEqual({ state: "gone" });
    expect(c.get(ID2)).toEqual({ state: "unknown" });
    c.get(ID); c.get(ID2);
    expect(asked).toEqual([ID, ID2]);
    expect(changes).toBe(2);
  });

  test("something that isn't a map id is unknown without asking", () => {
    const c = new PackageCache(async () => { throw new Error("asked"); });
    expect(c.get("lockout")).toEqual({ state: "unknown" });
  });

  test("a failure is asked again only after the retry time", async () => {
    let now = 0, n = 0;
    const c = new PackageCache(async () => { n++; if (n === 1) throw new Error("offline"); return { status: 200, body: { map: wire() } }; }, () => {}, () => now, 1000);
    c.get(ID);
    await tick();
    expect(c.get(ID)).toEqual({ state: "error" });
    now = 500;
    expect(c.get(ID).state).toBe("error");
    now = 1500;
    expect(c.get(ID).state).toBe("loading");
    await tick();
    expect(c.get(ID).state).toBe("ok");
    expect(n).toBe(2);
  });

  test("a list's packages are seeded so they aren't fetched", () => {
    const c = new PackageCache(async () => { throw new Error("asked"); });
    c.seed(parseMapSummaries([wire()]));
    expect(c.get(ID)).toMatchObject({ state: "ok", map: { name: "Lockdown Remix" } });
  });
});

describe("sessions with a custom map", () => {
  const session = (ad: Record<string, unknown>) => parseSession({
    session_id: "00112233445566aa", host_xuid: "0009000000000001", host_gamertag: "Sneaky Pete",
    advertisement: { slots: { public_filled: 2, public_open: 6 }, flags: "custom", map: "teacup", name: "Slayer", ...ad },
    state: "open", players: {},
  })!;

  test("the advertisement's map_id is read; absent or malformed is none", () => {
    expect(session({ map_id: ID }).map_id).toBe(ID);
    expect(session({ map_id: ID.toUpperCase() }).map_id).toBe(ID);
    expect(session({}).map_id).toBeNull();
    expect(session({ map_id: "lockout" }).map_id).toBeNull();
  });
});

describe("games on a custom map", () => {
  test("titles: the package's name, else the base map's", () => {
    expect(packageTitle(ok(), "Teacup")).toBe("Lockdown Remix");
    expect(packageTitle({ state: "loading" }, "Teacup")).toBe("Teacup");
    expect(packageTitle({ state: "gone" }, "Teacup")).toBe("Teacup");
    expect(packageTitle(null, "Teacup")).toBe("Teacup");
  });

  test("groups: keyed by map_id, labelled by the package or as modded", () => {
    expect(mapGroupKey({ map: "teacup" })).toBe("teacup");
    expect(mapGroupKey({ map: "teacup", map_id: null })).toBe("teacup");
    expect(mapGroupKey({ map: "teacup", map_id: ID })).toBe(ID);
    expect(mapGroupLabel(null, null, "Teacup")).toBe("Teacup");
    expect(mapGroupLabel(ID, ok(), "Teacup")).toBe("Lockdown Remix");
    expect(mapGroupLabel(ID, { state: "unknown" }, "Teacup")).toBe("Teacup (modded)");
  });

  test("the reducer keeps a game's map_id from events and snapshots", () => {
    resetPos();
    const s = initialState();
    applyEvents(s, [ev("GameStarted", "game-g1", { game_id: "g1", map: "teacup", map_id: ID, mode: "multiplayer", team_play: false, players: [] })]);
    expect(s.games.g1).toMatchObject({ map: "teacup", map_id: ID });
    applyGameSnapshot(s, { game_id: "g2", map: "teacup", mode: "multiplayer", started_at: "2026-11-16T10:00:00Z", players: [] });
    expect(s.games.g2!.map_id).toBeNull();
    applyGameSnapshot(s, { game_id: "g2", map: "teacup", map_id: ID2, mode: "multiplayer", started_at: "2026-11-16T10:00:00Z", players: [] });
    expect(s.games.g2!.map_id).toBe(ID2);
    applyGameSnapshot(s, { game_id: "g3", map: "teacup", map_id: "not-hex", mode: "multiplayer", started_at: "2026-11-16T10:00:00Z", players: [] });
    expect(s.games.g3!.map_id).toBeNull();
  });

  test("a modded level is its own row in the campaign record", () => {
    resetPos();
    const A = "0009000000000001";
    const s = initialState();
    const start = (id: string, extra: Record<string, unknown>, at: string) =>
      ev("GameStarted", `game-${id}`, { game_id: id, map: "harbor_dawn", mode: "campaign", team_play: false, tick: 0, players: [{ index: 0, name: "a", xuid: A, team: 0 }], ...extra }, at);
    applyEvents(s, [
      start("c1", {}, "2026-11-15T10:00:00.000Z"),
      ev("GameEnded", "game-c1", { tick: 900, reason: "finished" }, "2026-11-15T10:01:00.000Z"),
      start("c2", { map_id: ID }, "2026-11-15T11:00:00.000Z"),
      ev("GameEnded", "game-c2", { tick: 900, reason: "finished" }, "2026-11-15T11:01:00.000Z"),
    ]);
    const rows = campaignRecord(s, A, Date.parse("2026-11-15T12:00:00.000Z")).levels;
    expect(rows.map((l) => [l.map, l.map_id, l.plays])).toEqual([["harbor_dawn", ID, 1], ["harbor_dawn", null, 1]]);
  });
});

describe("pictures and the 3D view", () => {
  test("the package's image only when it has one", () => {
    expect(packageImageUrl("http://h/v1", ok({ image: "f6".repeat(32) }))).toBe(`http://h/v1/maps/${ID}/image`);
    expect(packageImageUrl("http://h/v1", ok())).toBeNull();
    expect(packageImageUrl("http://h/v1", { state: "gone" })).toBeNull();
  });

  test("first match wins: package image, then the base map's art", () => {
    expect(pictureChoice("pkg.png", "base.png")).toEqual({ src: "pkg.png", from: "package" });
    expect(pictureChoice(null, "base.png")).toEqual({ src: "base.png", from: "base" });
    expect(pictureChoice(null, null)).toBeNull();
  });

  test("base geometry unless the package changed it", () => {
    const g = "07".repeat(32);
    expect(geometryChoice(null)).toBe("base");
    expect(geometryChoice({ state: "loading" })).toBe("base");
    expect(geometryChoice(ok())).toBe("base");
    expect(geometryChoice(ok({ geometry: g }))).toBe("changed");
    expect(geometryChoice(ok({ geometry: g }), g)).toBe("base");
    expect(geometryChoice(ok({ geometry: g }), "18".repeat(32))).toBe("changed");
  });
});

describe("an admin's details", () => {
  test("screenshots, contents and the report read; absent ones are left out", () => {
    const m = parseMapSummary(wire({
      images: [
        { sha256: "01".repeat(32), caption: " The hog ", credit: "Bocked", source_url: "javascript:alert(1)" },
        { sha256: "02".repeat(32), source_url: "https://example.invalid/post" },
      ],
      contents: ["model swap: warthog uses the scorpion model", "  "],
      report: { lines: ["new vehicle placements: 4 banshees"], swaps: [{ kind: "model", tag: "objects\\v\\hog", field: "model", from: "hog", to: "tank" }], scripts: true },
    }))!;
    expect(m.images).toEqual([
      { sha256: "01".repeat(32), caption: "The hog", credit: "Bocked" },
      { sha256: "02".repeat(32), source_url: "https://example.invalid/post" },
    ]);
    expect(m.contents).toEqual(["model swap: warthog uses the scorpion model"]);
    expect(m.report).toMatchObject({ lines: ["new vehicle placements: 4 banshees"], scripts: true, classes: [], stats: [] });
    expect(m.report?.swaps[0]).toEqual({ kind: "model", tag: "objects\\v\\hog", field: "model", from: "hog", to: "tank" });
    const plain = parseMapSummary(wire())!;
    expect("images" in plain || "contents" in plain || "report" in plain).toBe(false);
    expect(parseMapSummary(wire({ report: {} }))!.report).toBeUndefined();
  });

  test("a list with an unreadable image keeps none, so the indexes stay the server's", () => {
    expect(parseMapSummary(wire({ images: [{ sha256: "01".repeat(32) }, { caption: "no hash" }] }))!.images).toBeUndefined();
    expect(packageScreenshotUrl("https://live.example/v1", ID, 2)).toBe(`https://live.example/v1/maps/${ID}/images/2`);
  });

  test("a pack's name and info read", () => {
    const m = parseMapSummary(wire({
      pack: "ai-revision", pack_name: " AI Revision ",
      pack_info: { id: "ai-revision", name: "AI Revision", authors: ["Zone 117", ""], released: "2007-06", source_url: "ftp://x", images: [{ sha256: "03".repeat(32) }] },
    }))!;
    expect(m.pack).toBe("ai-revision");
    expect(m.pack_name).toBe("AI Revision");
    expect(m.pack_info).toEqual({ id: "ai-revision", name: "AI Revision", authors: ["Zone 117"], released: "2007-06", images: [{ sha256: "03".repeat(32) }] });
    expect(parseMapSummary(wire({ pack_info: { name: "no id" } }))!.pack_info).toBeUndefined();
    expect(packImageUrl("https://live.example/v1", "ai-revision", 0)).toBe("https://live.example/v1/mappacks/ai-revision/images/0");
  });
});
