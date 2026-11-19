// Custom and modded maps on Live (docs/game/maps.md §5): the server's map
// packages (`GET /v1/maps`, `GET /v1/maps/{map_id}`), how a session or a
// game that names one is shown, and the fallbacks when the package is
// unknown, still loading or taken down. Everything here is pure; the
// per-page cache that asks the server lives in state/maps.ts.

/** A package id (a MapId): 64 lowercase hex. */
export type MapId = string;

export interface MapAuthor { xuid: string; gamertag: string }

export interface MapBase {
  /** The base map's name ("lockout"), as games and sessions carry `map`. */
  map: string;
  sha256?: string;
  /** True when the package is built on another package rather than retail. */
  derived: boolean;
}

export interface MapPackageLayer { sha256: string; size: number; kind: "patch" | "pack" }

/** `GET /v1/maps` items. */
export interface MapSummary {
  map_id: MapId;
  name: string;
  description?: string;
  author: MapAuthor;
  base: MapBase;
  layers: MapPackageLayer[];
  /** Bytes a player downloads: the layers' sizes summed. */
  size: number;
  /** The image's sha256, present exactly when `/image` serves one. */
  image?: string;
  /** The manifest's `[result] geometry`, only when the mod changed geometry. */
  geometry?: string;
  published_at: string;
  /** The package's log position (the list's `sort=new` order); 0 when absent. */
  position: number;
  /** Distinct signed-in accounts that fetched the manifest; 0 when absent. */
  downloads: number;
  /** Who made the mod (the original author or authors) on one line: never
   *  the publishing account, which is `author`. */
  credit?: string;
  /** The whole credit: authors, the original release and its date, where
   *  it came from, notes. Set by the publisher or corrected by an admin. */
  credits?: MapCredit;
  /** One of lib/mods.ts MOD_CATEGORIES, as the server sends it. */
  category?: string;
  /** Where the mod came from; kept only when it is http or https. */
  source_url?: string;
}

/** Who made a mod and where it came from (`credits` on a package). */
export interface MapCredit {
  authors: string[];
  release?: string;
  /** "2005", "2005-03" or "2005-03-14". */
  released?: string;
  source_url?: string;
  notes?: string;
}

/** `GET /v1/maps/{map_id}`'s `map`. */
export interface MapDetail extends MapSummary { manifest?: Record<string, unknown> }

export function isMapId(v: unknown): v is MapId {
  return typeof v === "string" && /^[0-9a-f]{64}$/.test(v);
}

const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null);
const text = (v: unknown): string | null => (typeof v === "string" && v.trim() ? v.trim() : null);
const bytes = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0);

/** A source URL fit to link: http or https only, else null. */
export function safeSourceUrl(v: unknown): string | null {
  const t = text(v);
  if (!t) return null;
  try {
    const u = new URL(t);
    return u.protocol === "http:" || u.protocol === "https:" ? u.toString() : null;
  } catch {
    return null;
  }
}

/** Reads one package as the server sends it; null without a valid id, a
 *  name and a base map. Optional fields are left out when absent. */
export function parseMapSummary(raw: unknown): MapDetail | null {
  const r = obj(raw);
  if (!r || !isMapId(r.map_id)) return null;
  const name = text(r.name), base = obj(r.base), author = obj(r.author);
  const baseMap = text(base?.map);
  if (!name || !baseMap) return null;
  const layers: MapPackageLayer[] = [];
  for (const l of Array.isArray(r.layers) ? r.layers : []) {
    const o = obj(l);
    if (!o || typeof o.sha256 !== "string") continue;
    layers.push({ sha256: o.sha256, size: bytes(o.size), kind: o.kind === "pack" ? "pack" : "patch" });
  }
  const out: MapDetail = {
    map_id: r.map_id,
    name,
    author: { xuid: text(author?.xuid) ?? "", gamertag: text(author?.gamertag) ?? "unknown" },
    base: { map: baseMap, derived: base?.derived === true },
    layers,
    size: bytes(r.size) || layers.reduce((n, l) => n + l.size, 0),
    published_at: text(r.published_at) ?? "",
    position: bytes(r.position),
    downloads: bytes(r.downloads),
  };
  const description = text(r.description);
  if (description) out.description = description;
  if (typeof base?.sha256 === "string") out.base.sha256 = base.sha256;
  if (typeof r.image === "string" && r.image) out.image = r.image;
  if (typeof r.geometry === "string" && r.geometry) out.geometry = r.geometry;
  const credit = text(r.credit), category = text(r.category), source = safeSourceUrl(r.source_url);
  if (credit) out.credit = credit;
  if (category) out.category = category.toLowerCase();
  if (source) out.source_url = source;
  const credits = parseMapCredit(r.credits);
  if (credits) out.credits = credits;
  if (obj(r.manifest)) out.manifest = r.manifest as Record<string, unknown>;
  return out;
}

/** A package's `credits`, read leniently: blank authors dropped, an unsafe
 *  source URL dropped; null when nothing is left. */
export function parseMapCredit(raw: unknown): MapCredit | null {
  const r = obj(raw);
  if (!r) return null;
  const authors = (Array.isArray(r.authors) ? r.authors : []).map(text).filter((a): a is string => a !== null);
  const out: MapCredit = { authors };
  const release = text(r.release), released = text(r.released), source = safeSourceUrl(r.source_url), notes = text(r.notes);
  if (release) out.release = release;
  if (released) out.released = released;
  if (source) out.source_url = source;
  if (notes) out.notes = notes;
  return authors.length || release || released || source || notes ? out : null;
}

/** A list response's items, dropping any that can't be read. */
export function parseMapSummaries(items: unknown): MapSummary[] {
  return Array.isArray(items) ? items.map(parseMapSummary).filter((m): m is MapDetail => m !== null) : [];
}

/** "812 B", "24 KB", "2.4 MB": one decimal under ten, whole numbers above. */
export function formatBytes(n: number): string {
  const v = Math.max(0, Math.floor(n));
  if (v < 1024) return `${v} B`;
  const units = ["KB", "MB", "GB"];
  let x = v / 1024, u = 0;
  while (x >= 1024 && u < units.length - 1) { x /= 1024; u++; }
  const s = x < 10 ? (Math.round(x * 10) / 10).toFixed(1) : String(Math.round(x));
  return `${s.replace(/\.0$/, "")} ${units[u]}`;
}

// ---- lookups: what the server said about one map_id ----

/** `ok`: the package. `unknown`: 404, not a package (it may be a retail
 *  map's id). `gone`: 410, hidden, deleted or blocked by an admin.
 *  `error`: no answer; asked again later. */
export type PackageLookup =
  | { state: "loading" }
  | { state: "ok"; map: MapDetail; missing: string[] }
  | { state: "unknown" }
  | { state: "gone" }
  | { state: "error" };

/** What `GET /v1/maps/{id}` answered, as a lookup. */
export function lookupFromResponse(status: number, body: unknown): PackageLookup {
  if (status === 404) return { state: "unknown" };
  if (status === 410) return { state: "gone" };
  if (status < 200 || status >= 300) return { state: "error" };
  const b = obj(body);
  const map = parseMapSummary(b?.map ?? body);
  if (!map) return { state: "error" };
  const missing = Array.isArray(b?.missing) ? (b!.missing as unknown[]).filter((s): s is string => typeof s === "string") : [];
  return { state: "ok", map, missing };
}

export type PackageFetch = (id: MapId) => Promise<{ status: number; body?: unknown }>;

/** Package lookups by map_id for one page load: each id is asked once; a
 *  404 or 410 is kept as the answer, a failure is asked again after
 *  `retryMs`. `changed` runs whenever an answer arrives. */
export class PackageCache {
  private entries = new Map<MapId, { lookup: PackageLookup; at: number }>();

  constructor(
    private fetcher: PackageFetch,
    private changed: () => void = () => {},
    private now: () => number = Date.now,
    private retryMs = 30_000,
  ) {}

  /** The lookup so far; starts the fetch the first time an id is seen. An
   *  id that isn't a MapId is `unknown` without asking. */
  get(id: MapId): PackageLookup {
    if (!isMapId(id)) return { state: "unknown" };
    const e = this.entries.get(id);
    if (e && !(e.lookup.state === "error" && this.now() - e.at >= this.retryMs)) return e.lookup;
    const loading: PackageLookup = { state: "loading" };
    this.entries.set(id, { lookup: loading, at: this.now() });
    void this.fetcher(id)
      .then((r) => lookupFromResponse(r.status, r.body), (): PackageLookup => ({ state: "error" }))
      .then((lookup) => { this.entries.set(id, { lookup, at: this.now() }); this.changed(); });
    return loading;
  }

  /** Remembers packages a list already returned, so they aren't asked for. */
  seed(maps: readonly MapSummary[]) {
    let any = false;
    for (const m of maps) {
      const had = this.entries.get(m.map_id)?.lookup;
      if (had?.state === "ok") continue;
      this.entries.set(m.map_id, { lookup: { state: "ok", map: m, missing: [] }, at: this.now() });
      any = true;
    }
    if (any) this.changed();
  }
}

// ---- how a session or game that names a map_id is shown ----

/** Grouping and filter key for a game's map: the package when it has one,
 *  so a modded Lockout never mixes into Lockout's stats. */
export function mapGroupKey(g: { map: string; map_id?: string | null }): string {
  return g.map_id || g.map;
}

/** The map's name where a game is shown: the package's, else the base map's
 *  title (unknown, taken down or still loading). */
export function packageTitle(lookup: PackageLookup | null | undefined, baseTitle: string): string {
  return lookup?.state === "ok" ? lookup.map.name : baseTitle;
}

/** A group's label (a filter option, a level row): the package's name, else
 *  "<Base title> (modded)" so it never reads as the retail map. */
export function mapGroupLabel(mapId: string | null | undefined, lookup: PackageLookup | null | undefined, baseTitle: string): string {
  if (!mapId) return baseTitle;
  return lookup?.state === "ok" ? lookup.map.name : `${baseTitle} (modded)`;
}

export type CustomMapBadge =
  | { kind: "custom"; name: string; author: string; size: number; download: string }
  | { kind: "unavailable"; text: string };

/** The custom games browser's badge for a session's map_id: the package
 *  and what joining downloads; "custom map unavailable" once it was taken
 *  down; nothing while loading, on no answer, or for an id the server
 *  doesn't know (a retail map's, shown plain). */
export function customMapBadge(mapId: string | null | undefined, lookup: PackageLookup | null | undefined): CustomMapBadge | null {
  if (!mapId || !lookup) return null;
  if (lookup.state === "gone") return { kind: "unavailable", text: "custom map unavailable" };
  if (lookup.state !== "ok") return null;
  const m = lookup.map;
  return { kind: "custom", name: m.name, author: m.author.gamertag, size: m.size, download: `needs download (${formatBytes(m.size)})` };
}

/** `/v1/maps/{id}/image`, only when the package says it has one. */
export function packageImageUrl(api: string, lookup: PackageLookup | null | undefined): string | null {
  return lookup?.state === "ok" && lookup.map.image ? `${api}/maps/${lookup.map.map_id}/image` : null;
}

/** A map_id's picture, first match wins: the package's own image, then
 *  the base map's art (preview or top view), else nothing. */
export function pictureChoice(packageImage: string | null | undefined, baseArt: string | null | undefined): { src: string; from: "package" | "base" } | null {
  if (packageImage) return { src: packageImage, from: "package" };
  if (baseArt) return { src: baseArt, from: "base" };
  return null;
}

/** Whether the 3D view can use the base map's geometry. `changed` when
 *  the package says its geometry changed: different from the base's hash
 *  when the art says the base's, else any `geometry` at all. */
export function geometryChoice(lookup: PackageLookup | null | undefined, baseGeometry?: string | null): "base" | "changed" {
  if (lookup?.state !== "ok" || !lookup.map.geometry) return "base";
  return baseGeometry && baseGeometry === lookup.map.geometry ? "base" : "changed";
}

export const GEOMETRY_CHANGED_NOTE = "geometry changed, no 3D view";
