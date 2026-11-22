// Made-up custom map packages for the mock (docs/game/maps.md §5), so the web's
// custom-map paths run without the game: `GET /v1/maps` and one package
// (`/v1/maps/{map_id}`, its `/image` as a drawn PNG), reports, hopper pins,
// and the admin's moderation (`/v1/admin/content`, hide / delete / block,
// the upload switch). Packages are invented on the mock's own maps; no
// bytes of any real map are involved. Some sessions and games name a
// package (`customFor`): one with a picture, one whose geometry changed, one
// an admin took down (410), and an id the server doesn't know (404). Eight
// more invented mods fill the repository browser (`/games/halo2/mods`), with
// categories, credits, source links and download counts; the list sorts,
// filters and pages with cursors as the server does.

import { createHash } from "node:crypto";
import { encodePng } from "./gameart";
import { parseJws } from "../src/state/verify";
import type { Admin, Reply } from "./admin";
import type { World } from "./world";

const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });
const sha = (s: string) => createHash("sha256").update(s).digest("hex");

type State = "ok" | "hidden" | "deleted" | "blocked";

interface MockPackage {
  map_id: string;
  name: string;
  description?: string;
  author: { xuid: string; gamertag: string };
  base: { map: string; derived: boolean };
  layers: { sha256: string; size: number; kind: "patch" | "pack" }[];
  image?: { sha256: string; png: Uint8Array };
  geometry?: string;
  published_at: string;
  state: State;
  /** The package's log position: newer is higher. */
  position: number;
  /** Downloads before the mock started; signed-in fetches add to it. */
  downloads: number;
  credit?: string;
  category?: Category;
  source_url?: string;
}

interface MockPackInfo {
  name: string;
  authors: string[];
  version?: string;
  description?: string;
  source_url?: string;
  year?: number;
  advisory?: "not_for_online" | "cheating_pack";
}

type Category = "gameplay" | "weapons" | "vehicles" | "visuals" | "maps" | "other";
const CATEGORIES: readonly string[] = ["gameplay", "weapons", "vehicles", "visuals", "maps", "other"];

/** More invented mods for the repository browser, on the mock's own maps,
 *  oldest first: [name, base, category, size, downloads, description, credit, image colours]. */
const MORE: Array<[string, string, Category, number, number, string?, string?, [number, number, number][]?]> = [
  ["Orchard Low Gravity", "orchard", "gameplay", 180_000, 412, "Everyone jumps twice as high. The trees are still in the way."],
  ["Lighthouse Fog Bank", "lighthouse", "visuals", 6_700_000, 88, "Thick sea fog rolls in from the water; the lamp cuts through it.", "Harbourmaster Gil", [[90, 100, 110], [200, 205, 210]]],
  ["Teacup Turret Party", "teacup", "weapons", 1_150_000, 1_204, "Mounted guns on every balcony. Bring a friend to carry the ammo.", undefined, [[150, 40, 40], [240, 200, 80]]],
  ["Orchard Ghost Run", "orchard", "vehicles", 3_400_000, 57, "Two fast hover bikes and a lap marked out between the rows.", "Plum Rider"],
  ["Teacup Rain", "teacup", "visuals", 9_100_000, 233, "Overcast sky, wet floors and rain on the windows."],
  ["Lighthouse Rocks", "lighthouse", "maps", 14_800_000, 640, "New rocks along the shore give the far side some cover.", undefined, [[30, 70, 60], [150, 170, 120]]],
  ["Orchard Snipers Only", "orchard", "weapons", 95_000, 1_204, "Long rifles on spawn, nothing else on the map."],
  ["Teacup Saucer Swap", "teacup", "other", 640_000, 19, "Swaps the saucer props for stacks of plates. That's all.", "Crockery Club", [[110, 60, 140], [230, 190, 220]]],
];


/** A small picture: a two-colour diagonal with a ring, nothing like a map. */
function picture(a: [number, number, number], b: [number, number, number]): Uint8Array {
  const w = 160, h = 90, px = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const d = Math.hypot(x - w / 2, y - h / 2);
    const c = Math.abs(d - 28) < 4 ? [255, 255, 255] : x / w + y / h < 1 ? a : b;
    px.set([c[0]!, c[1]!, c[2]!, 255], (y * w + x) * 4);
  }
  return encodePng(px, w, h);
}

function layer(seed: string, size: number, kind: "patch" | "pack" = "patch") {
  return { sha256: sha("layer " + seed), size, kind };
}

/** An id no package has: sessions with it show as a plain map. */
export const UNKNOWN_MAP_ID = sha("mock map nobody published");

/** The packages' ids and base maps, fixed so games seeded before the
 *  server starts can name them. */
const IDS = { crates: sha("mock package crates"), towers: sha("mock package towers"), party: sha("mock package taken down") };
const BASES: Array<[id: string, base: string]> = [[IDS.crates, "teacup"], [IDS.towers, "orchard"], [IDS.party, "lighthouse"]];

/** The package a session or game on `map` plays, if any: picked from its
 *  id so the mock's dice don't move. About a third name one; now and then
 *  it is an id no package has. */
export function customFor(key: string, map: string): string | undefined {
  const n = parseInt(sha(key).slice(0, 4), 16);
  if (n % 3) return undefined;
  if (n % 7 === 0) return UNKNOWN_MAP_ID;
  return BASES.find(([, base]) => base === map)?.[0];
}

export class Content {
  readonly packages: MockPackage[];
  private packState = new Map<string, State>();
  private reports: { target: "map" | "pack" | "content"; id: string; by: string; reason: string; at: string }[] = [];
  private blobState = new Map<string, State>();
  private uploadsOff = new Set<string>();
  private pins = new Map<number, string[]>();
  private packMembership = new Map<string, string>();
  private packMetadata = new Map<string, MockPackInfo>();

  constructor(private world: World, private admin: Admin) {
    const people = [...world.accounts.values()];
    const by = (i: number) => {
      const a = people[i % Math.max(1, people.length)];
      return a ? { xuid: a.xuid, gamertag: a.gamertag } : { xuid: "0009000000000001", gamertag: "Mapsmith" };
    };
    const at = (minAgo: number) => new Date(Date.now() - minAgo * 60_000).toISOString();
    const img = picture([40, 90, 140], [200, 150, 60]);
    this.packages = [
      {
        map_id: IDS.crates, name: "Teacup Crates", description: "More cover on the lower floor.",
        author: by(0), base: { map: "teacup", derived: false }, layers: [layer("crates", 2_400_000)],
        image: { sha256: createHash("sha256").update(img).digest("hex"), png: img }, published_at: at(300), state: "ok",
        position: 30, downloads: 96, category: "maps", credit: "Old Crate Co.", source_url: "https://mods.example.net/teacup-crates",
      },
      {
        map_id: IDS.towers, name: "Orchard Towers",
        author: by(0), base: { map: "orchard", derived: false }, layers: [layer("towers", 9_800_000), layer("towers pack", 3_100_000, "pack")],
        geometry: sha("mock towers geometry"), published_at: at(120), state: "ok", position: 31, downloads: 7, category: "maps",
        // Not a web link: the page must not show it.
        source_url: "javascript:alert(1)",
      },
      {
        map_id: IDS.party, name: "Lighthouse Party",
        author: by(2), base: { map: "lighthouse", derived: false }, layers: [layer("party", 512_000)],
        published_at: at(60), state: "blocked", position: 32, downloads: 3, category: "gameplay",
      },
      ...MORE.map(([name, map, category, size, downloads, description, credit, colours], i): MockPackage => {
        const png = colours ? picture(colours[0]!, colours[1]!) : undefined;
        const layers = size > 5_000_000 ? [layer(name, Math.round(size * 0.7)), layer(name + " pack", size - Math.round(size * 0.7), "pack")] : [layer(name, size)];
        return {
          map_id: sha("mock package " + name), name, ...(description ? { description } : {}), author: by(i + 3),
          base: { map, derived: false }, layers, ...(png ? { image: { sha256: createHash("sha256").update(png).digest("hex"), png } } : {}),
          published_at: at(60 * 24 * (MORE.length - i) + 400), state: "ok", position: 10 + i, downloads, category,
          ...(credit ? { credit } : {}), ...(i % 3 === 1 ? { source_url: `https://mods.example.net/${encodeURIComponent(name.toLowerCase().replace(/ /g, "-"))}` } : {}),
        };
      }),
    ];
    for (const p of this.packages) {
      const id = p.map_id === IDS.crates || p.map_id === IDS.towers ? "harbour-works" : p.map_id;
      this.packMembership.set(p.map_id, id);
      this.packMetadata.set(id, { name: p.name, authors: p.credit ? [p.credit] : [], description: p.description, source_url: p.source_url });
    }
    this.packMetadata.set("harbour-works", {
      name: "Harbour Works", authors: ["Old Crate Co.", "Harbour Club"], version: "2.0", year: 2006,
      description: "New cover and towers across the harbour.", source_url: "https://mods.example.net/harbour-works",
      advisory: "not_for_online",
    });
    this.reports.push({ target: "map", id: this.byId(IDS.party).map_id, by: by(3).gamertag, reason: "Not what it says it is.", at: at(50) });
    this.pins.set(1, [IDS.crates]);
  }

  private byId(id: string): MockPackage {
    return this.packages.find((p) => p.map_id === id)!;
  }

  /** Who has fetched each package's manifest with a token. */
  private fetchedBy = new Map<string, Set<string>>();

  private downloads(p: MockPackage): number {
    return p.downloads + (this.fetchedBy.get(p.map_id)?.size ?? 0);
  }

  private summary(p: MockPackage) {
    const id = this.packMembership.get(p.map_id);
    const pack = id ? this.packMetadata.get(id) : undefined;
    const authors = p.credit ? [p.credit] : pack?.authors ?? [];
    const source = p.source_url ?? pack?.source_url;
    return {
      mod_pack: id, pack_name: pack?.name,
      map_id: p.map_id, name: p.name, ...(p.description ? { description: p.description } : {}),
      author: p.author, base: p.base, layers: p.layers, size: p.layers.reduce((n, l) => n + l.size, 0),
      ...(p.image ? { image: p.image.sha256 } : {}), ...(p.geometry ? { geometry: p.geometry } : {}), published_at: p.published_at,
      position: p.position, downloads: this.downloads(p), ...(authors.length ? { credit: authors.join(", "), credits: { authors } } : {}),
      ...(p.category ? { category: p.category } : {}), ...(source ? { source_url: source } : {}),
    };
  }

  private packRows(includeUnavailable = false) {
    const packs = [];
    for (const [id, info] of this.packMetadata) {
      const maps = this.packages.filter((p) => this.packMembership.get(p.map_id) === id);
      if (!maps[0] || (!includeUnavailable && ((this.packState.get(id) ?? "ok") !== "ok" || maps.some((p) => p.state !== "ok")))) continue;
      const members = maps.map((m) => this.summary(m));
      packs.push({ ...info, id, members, uploader: maps[0].author, images: [], map_count: members.length,
        size: members.reduce((n, m) => n + m.size, 0), position: Math.max(...maps.map((p) => p.position)) });
    }
    return packs.sort((a, b) => b.position - a.position);
  }

  private async packRequest(req: Request, id: string, report: boolean): Promise<Reply> {
    const all = this.packRows(true).find((p) => p.id === id);
    if (!all) return fail(404, "not_found", "no such pack");
    const pack = this.packRows().find((p) => p.id === id);
    if (!pack) return fail(410, "gone", "this pack is unavailable");
    const position = this.world.log.length;
    if (!report && req.method === "GET") return ok({ pack, position });
    const by = this.who(req.headers.get("authorization"));
    if (!by) return fail(401, "unauthorized", "sign in first");
    if (report && req.method === "POST") {
      const body = await req.json().catch(() => null) as { reason?: unknown } | null;
      if (typeof body?.reason !== "string" || !body.reason.trim()) return fail(400, "invalid", "a report needs a reason");
      this.reports.unshift({ target: "pack", id, by, reason: body.reason.trim().slice(0, 500), at: new Date().toISOString() });
      return ok({ ok: true, position }, 201);
    }
    if (!report && req.method === "PUT") {
      if (pack.uploader.gamertag !== by || this.uploadsOff.has(by)) return fail(403, "forbidden", "uploads are not allowed");
      const body = await req.json().catch(() => null) as Partial<MockPackInfo & { members: string[] }> | null;
      if (!body?.name?.trim() || !Array.isArray(body.authors) || !body.authors.length ||
          !body.authors.every((a) => typeof a === "string" && a.trim()) || !Array.isArray(body.members) ||
          body.members.length !== pack.map_count || !pack.members.every((m) => body.members!.includes(m.map_id))) {
        return fail(400, "invalid", "invalid pack metadata or membership");
      }
      this.packMetadata.set(id, { name: body.name.trim(), authors: body.authors, version: body.version, description: body.description,
        source_url: body.source_url, year: body.year, advisory: body.advisory });
      return ok({ pack: this.packRows().find((p) => p.id === id), position });
    }
    return fail(405, "unsupported", "method not supported");
  }

  private listed(): MockPackage[] {
    const hidden = new Set(this.packRows(true)
      .filter((pack) => (this.packState.get(pack.id) ?? "ok") !== "ok")
      .flatMap((pack) => pack.members.map((member) => member.map_id)));
    return this.packages.filter((p) => p.state === "ok" && !hidden.has(p.map_id));
  }

  async handle(req: Request, u: URL, image: (bytes: Uint8Array) => Response): Promise<Reply | Response | null> {
    const path = u.pathname;
    const pos = this.world.log.length;
    let m: RegExpMatchArray | null;
    if (path === "/v1/mod-packs" && req.method === "GET") return ok({ items: this.packRows(), position: pos });
    if ((m = path.match(/^\/v1\/mod-packs\/([^/]+)(\/report)?$/))) return this.packRequest(req, m[1]!, !!m[2]);
    if (/^\/v1\/(mod-packs|maps)\/[^/]+\/screenshots\//.test(path)) {
      return fail(501, "mock", "Connect to a Live server to store screenshots.");
    }
    if (path === "/v1/maps" && req.method === "GET") return this.list(u.searchParams, pos);
    if ((m = path.match(/^\/v1\/maps\/([0-9a-f]{64})(\/image|\/report)?$/))) {
      const p = this.packages.find((x) => x.map_id === m![1]);
      if (!p) return fail(404, "not_found", "no such map");
      if (m[2] === "/report" && req.method === "POST") {
        const b = (await req.json().catch(() => null)) as { reason?: unknown } | null;
        const by = this.who(req.headers.get("authorization"));
        if (!by) return fail(401, "unauthorized", "sign in to report");
        this.reports.unshift({ target: "map", id: p.map_id, by, reason: String(b?.reason ?? ""), at: new Date().toISOString() });
        return ok({ ok: true, position: pos });
      }
      if (!this.listed().includes(p)) return fail(410, "gone", "this map is unavailable");
      if (m[2] === "/image") return p.image ? image(p.image.png) : fail(404, "not_found", "this map has no image");
      // A token here is the console fetching the manifest: a download.
      const fetcher = this.who(req.headers.get("authorization"));
      if (fetcher) this.fetchedBy.set(p.map_id, (this.fetchedBy.get(p.map_id) ?? new Set()).add(fetcher));
      return ok({ map: { ...this.summary(p), manifest: { name: p.name, base: p.base.map } }, missing: [], position: pos });
    }
    if (path === "/v1/hoppers/pins" && req.method === "GET") {
      return ok({ pins: [...this.pins.entries()].map(([hopper_id, map_ids]) => ({ hopper_id, map_ids })), position: pos });
    }
    if ((m = path.match(/^\/v1\/hoppers\/(\d+)\/pins$/)) && req.method === "PUT") {
      const c = this.admin.caller(req.headers.get("authorization"), "content.moderate");
      if (!c.ok) return c.reply;
      const b = (await req.json().catch(() => null)) as { map_ids?: unknown } | null;
      if (!Array.isArray(b?.map_ids)) return fail(400, "invalid", "{map_ids} expected");
      this.pins.set(Number(m[1]), b.map_ids.filter((x): x is string => typeof x === "string"));
      return ok({ ok: true, position: pos });
    }
    if (!path.startsWith("/v1/admin/content") && !path.startsWith("/v1/admin/maps/") && !path.startsWith("/v1/admin/mod-packs/") && !path.startsWith("/v1/admin/uploads/")) return null;
    const c = this.admin.caller(req.headers.get("authorization"), "content.moderate");
    if (!c.ok) return c.reply;
    if (path === "/v1/admin/content" && req.method === "GET") return ok(this.adminList());
    if ((m = path.match(/^\/v1\/admin\/mod-packs\/([^/]+)\/(hide|unhide|delete|block|unblock)$/)) && req.method === "POST") {
      const id = decodeURIComponent(m[1]!);
      if (!this.packRows(true).some((pack) => pack.id === id)) return fail(404, "not_found", "no such pack");
      const states = { hide: "hidden", unhide: "ok", delete: "deleted", block: "blocked", unblock: "deleted" } as const;
      this.packState.set(id, states[m[2] as keyof typeof states]);
      return ok({ ok: true, position: pos });
    }
    if ((m = path.match(/^\/v1\/admin\/(maps|content)\/([0-9a-f]{64})\/(hide|unhide|delete|block|unblock)$/)) && req.method === "POST") {
      const next: State = { hide: "hidden", unhide: "ok", delete: "deleted", block: "blocked", unblock: "ok" }[m[3] as "hide"] as State;
      if (m[1] === "maps") {
        const p = this.packages.find((x) => x.map_id === m![2]);
        if (!p) return fail(404, "not_found", "no such map");
        p.state = next;
      } else {
        if (!this.blobs().some((b) => b.sha256 === m![2])) return fail(404, "not_found", "no such content");
        this.blobState.set(m[2]!, next);
      }
      return ok({ ok: true, position: pos });
    }
    if ((m = path.match(/^\/v1\/admin\/uploads\/([^/]+)$/)) && req.method === "PUT") {
      const b = (await req.json().catch(() => null)) as { allowed?: unknown } | null;
      if (typeof b?.allowed !== "boolean") return fail(400, "invalid", "{allowed} expected");
      const tag = decodeURIComponent(m[1]!);
      if (b.allowed) this.uploadsOff.delete(tag); else this.uploadsOff.add(tag);
      return ok({ ok: true, position: pos });
    }
    return fail(404, "not_found", `no route ${req.method} ${path}`);
  }

  /** `GET /v1/maps`: sort=new|popular, base, author, category, q, limit,
   *  cursor, as the server answers it (docs/live/live-api.md). */
  private list(sp: URLSearchParams, pos: number): Reply {
    const sort = sp.get("sort") ?? "new";
    if (sort !== "new" && sort !== "popular") return fail(400, "invalid", "sort is new or popular");
    const base = sp.get("base")?.toLowerCase(), author = sp.get("author")?.toLowerCase();
    const category = sp.get("category")?.toLowerCase(), q = sp.get("q")?.toLowerCase();
    if (category && !CATEGORIES.includes(category)) return fail(400, "invalid", "unknown category");
    const limit = Math.min(1000, Math.max(1, Number(sp.get("limit") ?? 100) || 100));
    const cursor = sp.get("cursor");
    const key = (p: MockPackage): [number, number] => (sort === "popular" ? [this.downloads(p), p.position] : [0, p.position]);
    let after: [number, number] | null = null;
    if (cursor) {
      const parts = cursor.split(":").map(Number);
      after = sort === "popular" ? (parts.length === 2 ? [parts[0]!, parts[1]!] : null) : (parts.length === 1 ? [0, parts[0]!] : null);
      if (!after || after.some((n) => !Number.isFinite(n))) return fail(400, "invalid", "bad cursor");
    }
    const hit = (p: MockPackage) => !q || [p.name, p.description, p.credit, p.author.gamertag, p.base.map].some((s) => s?.toLowerCase().includes(q));
    const rows = this.listed()
      .filter((p) => (!base || p.base.map.toLowerCase() === base) && (!author || p.author.gamertag.toLowerCase() === author) && (!category || p.category === category) && hit(p))
      .map((p) => ({ p, k: key(p) }))
      .sort((a, b) => b.k[0] - a.k[0] || b.k[1] - a.k[1])
      .filter(({ k }) => !after || k[0] < after[0] || (k[0] === after[0] && k[1] < after[1]))
      .slice(0, limit);
    const last = rows.length === limit ? rows[rows.length - 1]! : null;
    const next = last ? (sort === "popular" ? `${last.k[0]}:${last.k[1]}` : String(last.k[1])) : null;
    return ok({ items: rows.map(({ p }) => this.summary(p)), next, position: pos });
  }

  /** The gamertag behind a bearer token (mock: parsed, not verified). */
  private who(auth: string | null): string | null {
    const claims = auth?.startsWith("Bearer ") ? (parseJws(auth.slice(7).trim())?.payload as { iss?: string } | undefined) : undefined;
    const xuid = /^acct:([0-9a-f]{16})@/.exec(claims?.iss ?? "")?.[1];
    return xuid ? this.world.accounts.get(xuid)?.gamertag ?? null : null;
  }

  private blobs() {
    return this.packages.flatMap((p) => p.layers.map((l) => ({ ...l, uploader: p.author, stored_at: p.published_at })));
  }

  private adminList() {
    const count = (target: "map" | "pack" | "content", id: string) => this.reports.filter((r) => r.target === target && r.id === id).length;
    return {
      packs: this.packRows(true).map((pack) => ({
        ...pack, state: this.packState.get(pack.id) ?? "ok",
        reports: pack.members.reduce((total, member) => total + count("map", member.map_id), count("pack", pack.id)),
      })),
      maps: this.packages.map((p) => ({ ...this.summary(p), state: p.state, reports: count("map", p.map_id) })),
      blobs: this.blobs().map((b) => ({ sha256: b.sha256, size: b.size, uploader: b.uploader, state: this.blobState.get(b.sha256) ?? "ok", reports: count("content", b.sha256), stored_at: b.stored_at })),
      reports: this.reports,
      uploads_off: [...this.uploadsOff],
      position: this.world.log.length,
    };
  }
}
