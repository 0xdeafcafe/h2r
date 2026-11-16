// The current server's game art (lib/gameart.ts), loaded once per server.
// `gameArt()` is null until it arrives, and stays null when the server has
// none: callers then use the art pack.

import { createEffect, createMemo, createRoot, createSignal } from "solid-js";
import { compositeEmblem, countKeys, emblemCacheKey, emblemPalette, normaliseMask, loadGameArt, type EmblemSpec, type GameArt, type GameArtRef } from "../lib/gameart";
import { live } from "./live";
import { loadCatalog } from "../admin/catalog";
import { titleCase } from "../lib/format";

const [art, setArt] = createSignal<GameArt | null>(null);
export const gameArt = art;

// Map names from the owner's disc (catalog.json, by map file stem), so labels
// read "Ivory Tower" rather than the file's "cyclotron".
const [mapNames, setMapNames] = createSignal<ReadonlyMap<string, string>>(new Map());

/** A map file stem (or path) as the stem catalog keys use. */
export function mapStem(map: string | null | undefined): string {
  return (map ?? "").split(/[\\/]/).pop()!.replace(/\.map$/i, "").trim().toLowerCase();
}

/** A map's display name: the disc's own name when the server has the
 *  catalog, else the file stem title-cased. Reactive. */
export function mapTitle(map: string | null | undefined): string {
  const stem = mapStem(map);
  return mapNames().get(stem) ?? titleCase(stem);
}

/** Test hook: set the stem-to-name table directly. */
export function setMapTitles(names: Record<string, string>) {
  setMapNames(new Map(Object.entries(names).map(([k, v]) => [mapStem(k), v])));
}

// A server without game art answers 404 and the browser logs each one, so a
// miss is remembered for a minute rather than asked again on every reload.
const MISS_TTL_MS = 60_000;
const missKey = (base: string) => "h2live.no-game-art." + base;
function missed(base: string): boolean {
  try { return Date.now() - Number(sessionStorage.getItem(missKey(base)) ?? 0) < MISS_TTL_MS; } catch { return false; }
}

let started = false;
/** Loads the manifest now and again whenever the server changes. */
export function startGameArt() {
  if (started) return;
  started = true;
  createRoot(() => {
    createEffect(() => {
      const base = live.server().base;
      setArt(null);
      setMapNames(new Map());
      if (missed(base)) return;
      void loadGameArt(base).then((a) => {
        if (live.server().base !== base) return;
        if (!a) { try { sessionStorage.setItem(missKey(base), String(Date.now())); } catch { /* private mode */ } }
        setArt(a);
        if (!a) return;
        void loadCatalog(base).then((c) => {
          if (live.server().base !== base || !c) return;
          const m = new Map<string, string>();
          for (const e of c.maps) if (e.key && e.name) m.set(mapStem(e.key), e.name);
          setMapNames(m);
        });
      });
    });
  });
}

/** A key's picture from the server's game art, or null. Reactive. */
export function gameImage(key: string | null | undefined): GameArtRef | null {
  return key ? art()?.get(key) ?? null : null;
}

// ---- emblem compositing (needs a canvas, so it lives here, not in lib) ----

/** The emblem layers the server has, or null when it has no usable set. */
export const emblemLayers = createRoot(() => createMemo(() => {
  const a = art();
  if (!a) return null;
  const fg = countKeys(a, "emblem/fg/", 64), bg = countKeys(a, "emblem/bg/", 32);
  return fg && bg ? { a, fg, bg, palette: emblemPalette(a.manifest) } : null;
}));

const emblems = new Map<string, Promise<string | null>>();

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

/** The coloured emblem as an image URL, or null when the server lacks the
 *  layers or they can't be read. Cached by server, layers and colours. */
export function emblemUrl(a: GameArt, spec: EmblemSpec): Promise<string | null> {
  const key = a.root + "|" + emblemCacheKey(spec);
  let p = emblems.get(key);
  if (!p) {
    const bg = a.get(`emblem/bg/${spec.bg}`), fg = a.get(`emblem/fg/${spec.fg}`);
    p = !bg || !fg ? Promise.resolve(null) : Promise.all([loadImage(bg.url), loadImage(fg.url)]).then(([b, f]) => {
      const w = b.naturalWidth, h = b.naturalHeight;
      const out = compositeEmblem(pixels(b, w, h), pixels(f, w, h), spec);
      const c = document.createElement("canvas");
      c.width = w; c.height = h;
      c.getContext("2d")!.putImageData(new ImageData(out as Uint8ClampedArray<ArrayBuffer>, w, h), 0, 0);
      return c.toDataURL("image/png");
    }).catch(() => null);
    emblems.set(key, p);
  }
  return p;
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
