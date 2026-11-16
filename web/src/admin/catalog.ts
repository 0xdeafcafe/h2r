// The playlist builder's catalog: the maps, playlist texts and built-in game
// variants the owner's own game ships, read from their disc by
// `h2 live assets extract` and served beside the art manifest
// (docs/live/live-admin.md, "The catalog"). Nothing of it is in this repo; the
// builder works without it and falls back to raw numbers.

import { gameArtRoot } from "../lib/gameart";
import type { MapSummary } from "../lib/maps";

export const CATALOG_VERSION = 1;

export interface CatalogMap {
  /** The record id that resolves to this map. */
  id: number;
  name: string;
  description: string;
  /** The map's file stem, as the art manifest keys previews. */
  key?: string;
}

export interface CatalogPlaylist {
  key: string;
  title: string;
  description: string;
}

export interface CatalogVariant {
  name: string;
  game_type: string;
  /** The game-engine index a record's variant carries. */
  engine: number;
}

export interface Catalog {
  version: number;
  source?: string;
  /** Multiplayer maps: the playlist builder's choices. */
  maps: CatalogMap[];
  /** Campaign levels (`kind: "campaign"`): names and keys only, never
   *  offered in playlists (their ids are map ids, not hopper records). */
  campaign: CatalogMap[];
  playlists: CatalogPlaylist[];
  variants: CatalogVariant[];
  skipped: Record<string, string>;
  /** Published map packages from the server's `GET /v1/maps`, not the
   *  disc (empty in catalog.json; `withCustom` fills it). A hopper can
   *  allow them by map_id (`hopper pins`), not as hopper records. */
  custom: CatalogCustomMap[];
}

/** A published custom map, as the playlist builder lists it. */
export interface CatalogCustomMap {
  map_id: string;
  name: string;
  author: string;
  /** The base map's name ("lockout"); `base_title` its catalog name when known. */
  base: string;
  base_title: string;
}

/** Game types by engine index, as a record's variant carries them. */
export const ENGINES: Record<number, { key: string; name: string }> = {
  1: { key: "ctf", name: "Capture the Flag" },
  2: { key: "slayer", name: "Slayer" },
  3: { key: "oddball", name: "Oddball" },
  4: { key: "king_of_the_hill", name: "King of the Hill" },
  7: { key: "juggernaut", name: "Juggernaut" },
  8: { key: "territories", name: "Territories" },
  9: { key: "assault", name: "Assault" },
};

export function engineOf(gameType: string): number | null {
  for (const [n, e] of Object.entries(ENGINES)) if (e.key === gameType) return Number(n);
  return null;
}

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const u32 = (v: unknown): number | null =>
  typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= 0xffff_ffff ? v : null;

/** Checks a catalog's shape, dropping malformed entries; null for anything
 *  this client cannot read. */
export function parseCatalog(raw: unknown): Catalog | null {
  if (!raw || typeof raw !== "object") return null;
  const c = raw as Record<string, unknown>;
  if (c.version !== CATALOG_VERSION) return null;
  const list = (v: unknown): Record<string, unknown>[] =>
    Array.isArray(v) ? v.filter((e): e is Record<string, unknown> => !!e && typeof e === "object") : [];
  const maps: CatalogMap[] = [];
  const campaign: CatalogMap[] = [];
  for (const m of list(c.maps)) {
    const id = u32(m.id);
    if (id === null || !str(m.name)) continue;
    const entry: CatalogMap = { id, name: str(m.name), description: str(m.description) };
    if (str(m.key)) entry.key = str(m.key);
    (m.kind === "campaign" ? campaign : maps).push(entry);
  }
  const playlists = list(c.playlists)
    .filter((p) => str(p.key) && str(p.title))
    .map((p) => ({ key: str(p.key), title: str(p.title), description: str(p.description) }));
  const variants: CatalogVariant[] = [];
  for (const v of list(c.variants)) {
    const engine = u32(v.engine);
    if (engine === null || !str(v.name)) continue;
    variants.push({ name: str(v.name), game_type: str(v.game_type), engine });
  }
  const skipped: Record<string, string> = {};
  if (c.skipped && typeof c.skipped === "object")
    for (const [k, v] of Object.entries(c.skipped)) if (typeof v === "string") skipped[k] = v;
  return { version: CATALOG_VERSION, source: str(c.source) || undefined, maps, campaign, playlists, variants, skipped, custom: [] };
}

/** The server's catalog, or null when it has none or won't give it to this
 *  browser (it is served to the server's own machine by default). */
export async function loadCatalog(serverBase: string): Promise<Catalog | null> {
  try {
    const r = await fetch(gameArtRoot(serverBase) + "catalog.json", { headers: { accept: "application/json" } });
    if (!r.ok) return null;
    return parseCatalog(await r.json());
  } catch {
    return null;
  }
}

/** The catalog's `custom` section from the server's packages, by name;
 *  each base map named as the disc names it when the catalog knows it. */
export function customSection(catalog: Pick<Catalog, "maps"> | null, packages: readonly MapSummary[]): CatalogCustomMap[] {
  const stem = (m: string) => m.split(/[\\/]/).pop()!.replace(/\.map$/i, "").toLowerCase();
  const baseTitle = (m: string) => catalog?.maps.find((c) => c.key && stem(c.key) === stem(m))?.name ?? m.replace(/[_-]+/g, " ");
  return packages
    .map((p) => ({ map_id: p.map_id, name: p.name, author: p.author.gamertag, base: p.base.map, base_title: baseTitle(p.base.map) }))
    .sort((a, b) => a.name.localeCompare(b.name) || a.map_id.localeCompare(b.map_id));
}

/** The catalog with its `custom` section filled in. */
export function withCustom(catalog: Catalog, packages: readonly MapSummary[]): Catalog {
  return { ...catalog, custom: customSection(catalog, packages) };
}

/** `GET /v1/hoppers/pins`: the custom maps (map_ids) each hopper allows. */
export function parsePins(body: unknown): Map<number, string[]> {
  const out = new Map<number, string[]>();
  const pins = body && typeof body === "object" ? (body as { pins?: unknown }).pins : null;
  for (const p of Array.isArray(pins) ? pins : []) {
    if (!p || typeof p !== "object") continue;
    const { hopper_id, map_ids } = p as { hopper_id?: unknown; map_ids?: unknown };
    if (u32(hopper_id) === null || !Array.isArray(map_ids)) continue;
    out.set(hopper_id as number, map_ids.filter((m): m is string => typeof m === "string" && /^[0-9a-f]{64}$/.test(m)));
  }
  return out;
}

/** A hopper's pins with one map turned on or off, in a stable order. */
export function togglePin(pins: readonly string[], mapId: string, on: boolean): string[] {
  const rest = pins.filter((m) => m !== mapId);
  return on ? [...rest, mapId].sort() : rest;
}

const norm = (s: string): string => s.toLowerCase().replace(/[^a-z0-9]+/g, "");

/** A map by display name (case and punctuation ignored). */
export function findMap(catalog: Catalog | null, name: string): CatalogMap | null {
  return catalog?.maps.find((m) => norm(m.name) === norm(name)) ?? null;
}

export function mapById(catalog: Catalog | null, id: number): CatalogMap | null {
  return catalog?.maps.find((m) => m.id === id) ?? null;
}

export function playlistText(catalog: Catalog | null, key: string): CatalogPlaylist | null {
  return catalog?.playlists.find((p) => p.key === key) ?? null;
}

export { norm as normaliseName };
