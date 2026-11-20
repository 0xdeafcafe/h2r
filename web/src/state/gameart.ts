// The current server's game art (lib/gameart.ts), loaded once per server.
// `gameArt()` is null until it arrives, and stays null when the server has
// none: callers then use the art pack.

import { createEffect, createMemo, createRoot, createSignal } from "solid-js";
import { mapDisplayName, mapKey, normaliseMask, loadGameArt, resolveMapStem, type GameArt, type GameArtRef, type MapNameSources } from "../lib/gameart";
import { live } from "./live";
import { ENGINES, loadCatalog, type Catalog } from "../admin/catalog";
import { isHalo1 } from "../map/views";

const [art, setArt] = createSignal<GameArt | null>(null);
export const gameArt = art;
const [ready, setReady] = createSignal(false);
export const gameArtReady = ready;

// Map names from the owner's disc (catalog.json, by map file stem), so labels
// read "Ivory Tower" rather than the file's "cyclotron".
const [mapNames, setMapNames] = createSignal<ReadonlyMap<string, string>>(new Map());

/** A map file stem (or path) as the stem catalog keys use. */
export function mapStem(map: string | null | undefined): string {
  return (map ?? "").split(/[\\/]/).pop()!.replace(/\.map$/i, "").trim().toLowerCase();
}

const nameSources = (): MapNameSources => ({ names: mapNames(), maps: art()?.manifest.maps });
const hasPreview = (stem: string) => !!art()?.has(mapKey(stem));

/** The stem the server's names and art know a map by (lib/gameart.ts
 *  `resolveMapStem`): Halo 1's bare "bloodgulch" finds `h1_bloodgulch`. Reactive. */
export function artStem(map: string | null | undefined): string {
  return resolveMapStem(mapStem(map), nameSources(), hasPreview);
}

/** A map's display name: the disc's own name when the server has the
 *  catalog (multiplayer maps and campaign levels alike), the art manifest's
 *  for a converted Halo 1 map, else the file stem title-cased, without a
 *  campaign level's ordering prefix ("01b_"). Reactive. */
export function mapTitle(map: string | null | undefined): string {
  return mapDisplayName(mapStem(map), nameSources(), hasPreview);
}

/** True when the map is a Halo 1 map converted to run in Halo 2: the art
 *  manifest says so, or its stem is `h1_<map>` (map/views.ts `isHalo1`).
 *  Reactive. */
export function isHalo1Map(map: string | null | undefined): boolean {
  const stem = artStem(map);
  return !!stem && isHalo1(stem, art()?.manifest.maps?.[stem]);
}

/** Test hook: set the game art directly. */
export function setGameArtForTest(a: GameArt | null) {
  setArt(a);
  setReady(true);
}

/** Test hook: set the stem-to-name table directly. */
export function setMapTitles(names: Record<string, string>) {
  setMapNames(new Map(Object.entries(names).map(([k, v]) => [mapStem(k), v])));
}

// A server without game art answers 404 and the browser logs each one, so a
// miss is remembered for a minute rather than asked again on every reload.
// The manifest and the catalog are remembered apart: either can be missing.
const MISS_TTL_MS = 60_000;
const missKey = (what: string, base: string) => `h2live.no-${what}.` + base;
function missed(what: string, base: string): boolean {
  try { return Date.now() - Number(sessionStorage.getItem(missKey(what, base)) ?? 0) < MISS_TTL_MS; } catch { return false; }
}
function miss(what: string, base: string) {
  try { sessionStorage.setItem(missKey(what, base), String(Date.now())); } catch { /* private mode */ }
}

// Game types of the disc's built-in variants (catalog.json), by lower-case
// variant name: "rockets" is Slayer.
const [variantTypes, setVariantTypes] = createSignal<ReadonlyMap<string, string>>(new Map());

/** The game type's name of a variant the disc ships ("Rockets": "Slayer"),
 *  from the server's catalog; null for a variant it doesn't list (a
 *  player's own) or without the catalog. Reactive. */
export function variantType(variant: string | null | undefined): string | null {
  return variant ? variantTypes().get(variant.trim().toLowerCase()) ?? null : null;
}

/** Takes the catalog's map names and variant types (null clears them). */
export function applyCatalog(c: Pick<Catalog, "maps" | "campaign" | "variants"> | null) {
  const names = new Map<string, string>();
  const types = new Map<string, string>();
  for (const e of [...(c?.maps ?? []), ...(c?.campaign ?? [])]) if (e.key && e.name) names.set(mapStem(e.key), e.name);
  for (const v of c?.variants ?? []) {
    const t = ENGINES[v.engine]?.name;
    if (t) types.set(v.name.trim().toLowerCase(), t);
  }
  setMapNames(names);
  setVariantTypes(types);
}

type Loaders = {
  art: (base: string) => Promise<GameArt | null>;
  catalog: (base: string) => Promise<Catalog | null>;
  current: () => string;
};

/** Loads a server's art and its catalog, each on its own: a catalog is
 *  used even when the manifest is missing or unreadable, so names don't
 *  hang on the pictures. Answers for another server (the user switched)
 *  are dropped. */
export async function loadServerArt(base: string, load: Loaders): Promise<void> {
  setReady(false);
  setArt(null);
  applyCatalog(null);
  await Promise.all([
    (missed("game-art", base) ? Promise.resolve(null) : load.art(base)).then((a) => {
      if (load.current() !== base) return;
      if (!a) miss("game-art", base);
      setArt(a);
    }),
    missed("catalog", base) ? null : load.catalog(base).then((c) => {
      if (load.current() !== base) return;
      if (!c) miss("catalog", base);
      applyCatalog(c);
    }),
  ]);
}

let started = false;
/** Loads the manifest and catalog now and again whenever the server changes. */
export function startGameArt() {
  if (started) return;
  started = true;
  createRoot(() => {
    // The base, not the target: discovery replaces the target with one at
    // the same base, which must not load the art again.
    const serverBase = createMemo(() => live.server().base);
    createEffect(() => {
      void loadServerArt(serverBase(), { art: loadGameArt, catalog: loadCatalog, current: () => live.server().base });
    });
  });
}

/** A key's picture from the server's game art, or null. Reactive. */
export function gameImage(key: string | null | undefined): GameArtRef | null {
  return key ? art()?.get(key) ?? null : null;
}

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    // The pixels are read back, so a server on another origin must allow it.
    img.crossOrigin = "anonymous";
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("could not load " + url));
    img.src = url;
  });
}

function pixels(img: HTMLImageElement, w: number, h: number): Uint8ClampedArray {
  const c = document.createElement("canvas");
  c.width = w; c.height = h;
  const ctx = c.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(img, 0, 0, w, h);
  return ctx.getImageData(0, 0, w, h).data;
}

// ---- single-mask pictures (weapons) ----

const masks = new Map<string, Promise<string | null>>();

/** A mask picture made ready for CSS `mask` (see normaliseMask), or null
 *  when it can't be read. Cached by URL. */
export function maskUrl(ref: GameArtRef): Promise<string | null> {
  let p = masks.get(ref.url);
  if (!p) {
    p = loadImage(ref.url).then((img) => {
      const w = img.naturalWidth, h = img.naturalHeight;
      const c = document.createElement("canvas");
      c.width = w; c.height = h;
      c.getContext("2d")!.putImageData(new ImageData(normaliseMask(pixels(img, w, h)) as Uint8ClampedArray<ArrayBuffer>, w, h), 0, 0);
      return c.toDataURL("image/png");
    }).catch(() => null);
    masks.set(ref.url, p);
  }
  return p;
}
