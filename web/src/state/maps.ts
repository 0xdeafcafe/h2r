// Map packages on the current server, looked up by map_id as pages need
// them (lib/maps.ts has the rules). One fetch per id per page load, kept
// until the server changes; reads are reactive and return `loading` until
// the answer is in.

import { createSignal } from "solid-js";
import { live } from "./live";
import { isHalo1Map, mapTitle } from "./gameart";
import { PackageCache, geometryChoice, mapGroupLabel, packageImageUrl, packageTitle, type MapSummary, type PackageLookup } from "../lib/maps";

const [version, setVersion] = createSignal(0);
let cache: PackageCache | null = null;
let cacheFor = "";

function current(): PackageCache {
  const api = live.server().api;
  if (!cache || cacheFor !== api) {
    cacheFor = api;
    const mine = new PackageCache((id) => live.api().mapPackage(id), () => { if (cache === mine) setVersion((v) => v + 1); });
    cache = mine;
  }
  return cache;
}

/** What the server says about a map_id; null for none. Reactive. */
export function packageLookup(mapId: string | null | undefined): PackageLookup | null {
  if (!mapId) return null;
  version();
  return current().get(mapId);
}

/** Remembers packages a list returned (the catalog, the admin list). */
export function seedPackages(maps: readonly MapSummary[]) {
  current().seed(maps);
}

type GameMap = { map: string; map_id?: string | null };

/** The map's name for a game: its package's name, else the base map's
 *  title. Reactive. */
export function gameMapTitle(g: GameMap): string {
  return packageTitle(packageLookup(g.map_id), mapTitle(g.map));
}

/** The label of a group of games keyed by `mapGroupKey`: the package's
 *  name, else "<Base title> (modded)". Reactive. */
export function gameMapGroupLabel(g: GameMap): string {
  // A Halo 1 map is served as a package of the import's, not a mod of a
  // Halo 2 map: its own name, unless the package was published.
  const lookup = packageLookup(g.map_id);
  if (isHalo1Map(g.map) && lookup?.state !== "ok") return mapTitle(g.map);
  return mapGroupLabel(g.map_id, lookup, mapTitle(g.map));
}

/** The package's own image URL, when it has one. Reactive. */
export function packageImage(mapId: string | null | undefined): string | null {
  return packageImageUrl(live.server().api, packageLookup(mapId));
}

/** True when the game's package changed the level's geometry, so the base
 *  map's glb would be wrong: the 3D view says so instead. The art manifest
 *  carries no geometry hash for the base, so any `geometry` counts. Reactive. */
export function geometryChanged(g: GameMap): boolean {
  // A Halo 1 map's geometry is drawn from the converted map itself.
  if (isHalo1Map(g.map)) return false;
  return geometryChoice(packageLookup(g.map_id)) === "changed";
}
