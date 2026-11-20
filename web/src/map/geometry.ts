// A map's geometry files on the current server: the `map.glb` for the 3D
// viewer and the pre-drawn views for the 2D map. Both are optional; null
// means "use the fitted grid". Loaded once per server and map.

import { createEffect, createSignal, type Accessor } from "solid-js";
import { gameArt } from "../state/gameart";
import { mapFilesFor, parseViews, viewImageFile, type MapViews } from "./views";
import { fetchFeatures, type MapFeatures } from "./features";
import { modelUrl } from "../viewer/models";

export interface MapGeometry {
  /** URL of the map's glb, or null when the server has none. */
  glb: string | null;
  /** The pre-drawn views, or null when the server has none. */
  views: MapViews | null;
  /** URL of a view's picture. */
  image(id: string): string;
}

// Every file carries the art's version stamp, so the server can let the
// browser keep it (docs/live/live-art.md, "Caching").
const url = (root: string, file: string, stamp?: string) => modelUrl(root, file, stamp);

/** The 3D address is available as soon as the manifest is known. */
export function mapGlb(map: string, id: string | null): string | null {
  const art = gameArt();
  const file = art && mapFilesFor(art.manifest.maps, map, id)?.files.geometry;
  return art && file ? url(art.root, file, art.manifest.source) : null;
}

const viewsCache = new Map<string, Promise<MapViews | null>>();
function loadViews(u: string): Promise<MapViews | null> {
  let p = viewsCache.get(u);
  if (!p) {
    p = fetch(u).then((r) => (r.ok ? r.json() : null)).then(parseViews).catch(() => null);
    viewsCache.set(u, p);
  }
  return p;
}

/** The geometry for a map; null until known, and when the server has none.
 *  With the game's MapId, an installed mod's own geometry when the art has
 *  it (views.ts `mapFilesFor`), else the map name's. */
export function geometryFor(
  map: Accessor<string | undefined>,
  mapId: Accessor<string | null | undefined> = () => null,
): Accessor<MapGeometry | null> {
  const [geo, set] = createSignal<MapGeometry | null>(null);
  createEffect(() => {
    const art = gameArt(), m = map(), id = mapId();
    set(null);
    const files = art && m ? mapFilesFor(art.manifest.maps, m, id)?.files : undefined;
    if (!art || !files) return;
    const stamp = art.manifest.source;
    const glb = files.geometry ? url(art.root, files.geometry, stamp) : null;
    const still = () => gameArt() === art && map() === m && mapId() === id;
    if (!files.views) { set({ glb, views: null, image: () => "" }); return; }
    const viewsFile = files.views;
    void loadViews(url(art.root, viewsFile, stamp)).then((views) => {
      if (!still()) return;
      const image = (id: string) => url(art.root, viewImageFile(viewsFile, views?.views.find((v) => v.id === id) ?? { id }), stamp);
      set(views || glb ? { glb, views, image } : null);
    });
  });
  return geo;
}

const featureCache = new Map<string, Promise<MapFeatures | null>>();

/** A map's teleporters and lifts, read from the head of its glb; null
 *  until known, and when it has none. */
export function featuresFor(glb: Accessor<string | null | undefined>): Accessor<MapFeatures | null> {
  const [features, set] = createSignal<MapFeatures | null>(null);
  createEffect(() => {
    const u = glb();
    set(null);
    if (!u) return;
    let p = featureCache.get(u);
    if (!p) { p = fetchFeatures(u); featureCache.set(u, p); }
    void p.then((f) => { if (glb() === u) set(f); });
  });
  return features;
}
