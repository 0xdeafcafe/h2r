// Custom content for admins (docs/maps.md §5): the playlist builder's
// `custom` catalog section and hopper pins, and moderation (packages,
// blobs, reports, the upload switch). Every id, name and size is invented.

import { describe, expect, test } from "bun:test";
import { customSection, parseCatalog, parsePins, togglePin, withCustom } from "../src/admin/catalog";
import { actionsFor, fetchContent, moderate, moderatePath, parseContent, reportMap, reportsFor, setUploads, shortHash } from "../src/admin/content";
import { parseMapSummaries } from "../src/lib/maps";
import type { LocalAccount } from "../src/state/account";
import { parseJws } from "../src/state/verify";

const ID = "a1".repeat(32), ID2 = "b2".repeat(32), BLOB = "c3".repeat(32);

const pkg = (map_id: string, name: string, base = "teacup") => ({
  map_id, name, author: { xuid: "0009000000000001", gamertag: "Sneaky Pete" },
  base: { map: base, derived: false }, layers: [{ sha256: BLOB, size: 4096, kind: "patch" }], size: 4096, published_at: "2026-11-16T10:00:00Z",
});

const RFC = { d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo", kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k" };
const me = (): LocalAccount => ({ xuid: "0009000000000042", gamertag: "afr", id: "acct:0009000000000042@localhost", server: "http://127.0.0.1:8074", server_id: "localhost", kid: RFC.kid, key: RFC.d, x: RFC.x });

function recorder(reply: (url: string, init: RequestInit) => Response) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = (async (url: string, init: RequestInit = {}) => { calls.push({ url, init }); return reply(url, init); }) as unknown as typeof fetch;
  return { f, calls };
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const lxm = (init: RequestInit) => (parseJws((init.headers as Record<string, string>).authorization!.slice(7))!.payload as { lxm: string }).lxm;

describe("the playlist builder's custom section", () => {
  const catalog = parseCatalog({ version: 1, maps: [{ id: 3, name: "Teacup Arena", key: "teacup" }] })!;

  test("a catalog from the disc starts with no custom maps", () => {
    expect(catalog.custom).toEqual([]);
  });

  test("packages listed by name, with the base map's catalog name when known", () => {
    const maps = parseMapSummaries([pkg(ID2, "Zigzag"), pkg(ID, "Annex", "cliffside")]);
    expect(customSection(catalog, maps)).toEqual([
      { map_id: ID, name: "Annex", author: "Sneaky Pete", base: "cliffside", base_title: "cliffside" },
      { map_id: ID2, name: "Zigzag", author: "Sneaky Pete", base: "teacup", base_title: "Teacup Arena" },
    ]);
    expect(withCustom(catalog, maps).custom).toHaveLength(2);
    expect(customSection(null, maps)[1]!.base_title).toBe("teacup");
  });

  test("pins: read per hopper, dropping malformed ids; toggled in a stable order", () => {
    const pins = parsePins({ pins: [{ hopper_id: 2, map_ids: [ID2, "nope", ID] }, { hopper_id: -1, map_ids: [] }, { map_ids: [ID] }], position: 4 });
    expect([...pins.entries()]).toEqual([[2, [ID2, ID]]]);
    expect(parsePins(null).size).toBe(0);
    expect(togglePin([ID2], ID, true)).toEqual([ID, ID2]);
    expect(togglePin([ID, ID2], ID, false)).toEqual([ID2]);
    expect(togglePin([ID], ID, true)).toEqual([ID]);
  });
});

const CONTENT = {
  maps: [{ ...pkg(ID, "Annex"), state: "hidden", reports: 2 }, { map_id: "bad" }],
  blobs: [{ sha256: BLOB, size: 4096, uploader: { xuid: "0009000000000001", gamertag: "Sneaky Pete" }, state: "ok", reports: 0, stored_at: "2026-11-16T09:00:00Z" }],
  reports: [
    { target: "map", id: ID, by: "Grunt Birthday", reason: "older", at: "2026-11-16T09:00:00Z" },
    { target: "map", id: ID, by: "Noble Six", reason: "newer", at: "2026-11-16T11:00:00Z" },
    { target: "content", id: BLOB, by: "Noble Six", reason: "blob", at: "2026-11-16T10:00:00Z" },
  ],
  uploads_off: ["Spammy"],
  position: 99,
};

describe("moderation", () => {
  test("reads packages, blobs, reports (newest first) and the upload switch", () => {
    const c = parseContent(CONTENT);
    expect(c.maps.map((m) => [m.name, m.state, m.reports])).toEqual([["Annex", "hidden", 2]]);
    expect(c.blobs[0]).toMatchObject({ sha256: BLOB, state: "ok", uploader: { gamertag: "Sneaky Pete" } });
    expect(c.reports.map((r) => r.reason)).toEqual(["newer", "blob", "older"]);
    expect(reportsFor(c, "map", ID).map((r) => r.by)).toEqual(["Noble Six", "Grunt Birthday"]);
    expect(c.uploads_off).toEqual(["Spammy"]);
    expect(c.position).toBe(99);
    expect(parseContent(null)).toEqual({ maps: [], blobs: [], reports: [], uploads_off: [], position: null });
  });

  test("actions offered follow the state", () => {
    expect(actionsFor("ok")).toEqual(["hide", "delete", "block"]);
    expect(actionsFor("hidden")).toEqual(["unhide", "delete", "block"]);
    expect(actionsFor("deleted")).toEqual(["block"]);
    expect(actionsFor("blocked")).toEqual(["unblock"]);
  });

  test("paths for packages and blobs", () => {
    expect(moderatePath("map", ID, "hide")).toBe(`/admin/maps/${ID}/hide`);
    expect(moderatePath("content", BLOB, "unblock")).toBe(`/admin/content/${BLOB}/unblock`);
    expect(shortHash(BLOB)).toBe("c3c3c3c3…");
  });

  test("calls carry a content.moderate token; a server without content is null", async () => {
    const { f, calls } = recorder((url) => (url.endsWith("/admin/content") ? json(CONTENT) : json({ ok: true, position: 100 })));
    expect((await fetchContent("http://h/v1", "localhost", me(), f))!.maps).toHaveLength(1);
    await moderate("http://h/v1", "localhost", me(), "map", ID, "block", f);
    await setUploads("http://h/v1", "localhost", me(), "Spammy Tag", true, f);
    expect(calls.map((c) => [c.init.method ?? "GET", c.url])).toEqual([
      ["GET", "http://h/v1/admin/content"],
      ["POST", `http://h/v1/admin/maps/${ID}/block`],
      ["PUT", "http://h/v1/admin/uploads/Spammy%20Tag"],
    ]);
    expect(calls.map((c) => lxm(c.init))).toEqual(["content.moderate", "content.moderate", "content.moderate"]);
    expect(JSON.parse(calls[2]!.init.body as string)).toEqual({ allowed: true });
    const none = recorder(() => json({ error: "not_found" }, 404));
    expect(await fetchContent("http://h/v1", "localhost", me(), none.f)).toBeNull();
    const refused = recorder(() => json({ error: "forbidden", message: "admins only" }, 403));
    await expect(moderate("http://h/v1", "localhost", me(), "map", ID, "hide", refused.f)).rejects.toMatchObject({ status: 403, code: "forbidden" });
  });

  test("anyone signed in reports a package with a content.write token", async () => {
    const { f, calls } = recorder(() => json({ ok: true }));
    await reportMap("http://h/v1", "localhost", me(), ID, "  breaks the lift  ", f);
    expect(calls[0]!.url).toBe(`http://h/v1/maps/${ID}/report`);
    expect(lxm(calls[0]!.init)).toBe("content.write");
    expect(JSON.parse(calls[0]!.init.body as string)).toEqual({ reason: "breaks the lift" });
  });
});

describe("the mock's packages", async () => {
  const { Content, UNKNOWN_MAP_ID, customFor } = await import("../mock/content");
  const { Admin } = await import("../mock/admin");
  const { World } = await import("../mock/world");
  const world = new World();
  const content = new Content(world, new Admin(world, false));
  const get = async (path: string) => {
    const r = await content.handle(new Request("http://m" + path), new URL("http://m" + path), (b) => new Response(b as Uint8Array<ArrayBuffer>, { headers: { "content-type": "image/png" } }));
    return r instanceof Response ? { status: r.status, body: null } : r!;
  };

  test("lists the published packages and answers 200, 410 and 404 like the server", async () => {
    const list = await get("/v1/maps");
    const items = parseMapSummaries((list.body as { items: unknown[] }).items);
    expect(items.map((m) => m.name)).toEqual(["Orchard Towers", "Teacup Crates"]);
    const crates = items.find((m) => m.name === "Teacup Crates")!;
    expect((await get(`/v1/maps/${crates.map_id}`)).status).toBe(200);
    expect((await get(`/v1/maps/${crates.map_id}/image`)).status).toBe(200);
    expect((await get(`/v1/maps/${content.packages[2]!.map_id}`)).status).toBe(410);
    expect((await get(`/v1/maps/${UNKNOWN_MAP_ID}`)).status).toBe(404);
    expect(await get("/v1/maps/teacup/outline")).toBeNull();
    expect((await get("/v1/admin/content")).status).toBe(401);
  });

  test("custom maps are picked from the id, on the right base, about a third of the time", () => {
    const picks = Array.from({ length: 300 }, (_, i) => customFor(`g${i}`, "teacup"));
    const named = picks.filter(Boolean);
    expect(named.length).toBeGreaterThan(60);
    expect(named.length).toBeLessThan(140);
    expect(new Set(named).size).toBe(2); // the teacup package and the unknown id
    expect(customFor("g1", "teacup")).toBe(customFor("g1", "teacup"));
  });
});
