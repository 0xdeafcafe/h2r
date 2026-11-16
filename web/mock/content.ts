// Made-up custom map packages for the mock (docs/game/maps.md §5), so the web's
// custom-map paths run without the game: `GET /v1/maps` and one package
// (`/v1/maps/{map_id}`, its `/image` as a drawn PNG), reports, hopper pins,
// and the admin's moderation (`/v1/admin/content`, hide / delete / block,
// the upload switch). Packages are invented on the mock's own maps; no
// bytes of any real map are involved. Some sessions and games name a
// package (`customFor`): one with a picture, one whose geometry changed, one
// an admin took down (410), and an id the server doesn't know (404).

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
}

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
  private reports: { target: "map" | "content"; id: string; by: string; reason: string; at: string }[] = [];
  private blobState = new Map<string, State>();
  private uploadsOff = new Set<string>();
  private pins = new Map<number, string[]>();

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
      },
      {
        map_id: IDS.towers, name: "Orchard Towers",
        author: by(1), base: { map: "orchard", derived: false }, layers: [layer("towers", 9_800_000), layer("towers pack", 3_100_000, "pack")],
        geometry: sha("mock towers geometry"), published_at: at(120), state: "ok",
      },
      {
        map_id: IDS.party, name: "Lighthouse Party",
        author: by(2), base: { map: "lighthouse", derived: false }, layers: [layer("party", 512_000)],
        published_at: at(60), state: "blocked",
      },
    ];
    this.reports.push({ target: "map", id: this.packages[2]!.map_id, by: by(3).gamertag, reason: "Not what it says it is.", at: at(50) });
    this.pins.set(1, [this.packages[0]!.map_id]);
  }

  private summary(p: MockPackage) {
    return {
      map_id: p.map_id, name: p.name, ...(p.description ? { description: p.description } : {}),
      author: p.author, base: p.base, layers: p.layers, size: p.layers.reduce((n, l) => n + l.size, 0),
      ...(p.image ? { image: p.image.sha256 } : {}), ...(p.geometry ? { geometry: p.geometry } : {}), published_at: p.published_at,
    };
  }

  private listed(): MockPackage[] {
    return this.packages.filter((p) => p.state === "ok");
  }

  async handle(req: Request, u: URL, image: (bytes: Uint8Array) => Response): Promise<Reply | Response | null> {
    const path = u.pathname;
    const pos = this.world.log.length;
    let m: RegExpMatchArray | null;
    if (path === "/v1/maps" && req.method === "GET") {
      const base = u.searchParams.get("base"), author = u.searchParams.get("author")?.toLowerCase();
      const items = this.listed().filter((p) => (!base || p.base.map === base) && (!author || p.author.gamertag.toLowerCase() === author))
        .sort((a, b) => b.published_at.localeCompare(a.published_at)).map((p) => this.summary(p));
      return ok({ items, position: pos });
    }
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
      if (p.state !== "ok") return fail(410, "gone", `this map was ${p.state} by an admin`);
      if (m[2] === "/image") return p.image ? image(p.image.png) : fail(404, "not_found", "this map has no image");
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
    if (!path.startsWith("/v1/admin/content") && !path.startsWith("/v1/admin/maps/") && !path.startsWith("/v1/admin/uploads/")) return null;
    const c = this.admin.caller(req.headers.get("authorization"), "content.moderate");
    if (!c.ok) return c.reply;
    if (path === "/v1/admin/content" && req.method === "GET") return ok(this.adminList());
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
    const count = (target: "map" | "content", id: string) => this.reports.filter((r) => r.target === target && r.id === id).length;
    return {
      maps: this.packages.map((p) => ({ ...this.summary(p), state: p.state, reports: count("map", p.map_id) })),
      blobs: this.blobs().map((b) => ({ sha256: b.sha256, size: b.size, uploader: b.uploader, state: this.blobState.get(b.sha256) ?? "ok", reports: count("content", b.sha256), stored_at: b.stored_at })),
      reports: this.reports,
      uploads_off: [...this.uploadsOff],
      position: this.world.log.length,
    };
  }
}
