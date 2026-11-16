// A map's geometry files on the current server: the `map.glb` for the 3D
// viewer and the pre-drawn views for the 2D map. Both are optional; null
// means "use the fitted grid". Loaded once per server and map.

import { createEffect, createSignal, type Accessor } from "solid-js";
import { gameArt } from "../state/gameart";
import { mapStem, parseViews, viewImageFile, type MapViews } from "./views";

export interface MapGeometry {
  /** URL of the map's glb, or null when the server has none. */
  glb: string | null;
  /** The pre-drawn views, or null when the server has none. */
  views: MapViews | null;
  /** URL of a view's picture. */
  image(id: string): string;
}

const url = (root: string, file: string) => root + file.split("/").map(encodeURIComponent).join("/");

const viewsCache = new Map<string, Promise<MapViews | null>>();
function loadViews(u: string): Promise<MapViews | null> {
  let p = viewsCache.get(u);
  if (!p) {
    p = fetch(u).then((r) => (r.ok ? r.json() : null)).then(parseViews).catch(() => null);
    viewsCache.set(u, p);
  }
  return p;
}

/** The geometry for a map; null until known, and when the server has none. */
export function geometryFor(map: Accessor<string | undefined>): Accessor<MapGeometry | null> {
  const [geo, set] = createSignal<MapGeometry | null>(null);
  createEffect(() => {
    const art = gameArt(), m = map();
    set(null);
    const files = art && m ? art.manifest.maps?.[mapStem(m)] : undefined;
    if (!art || !files) return;
    const glb = files.geometry ? url(art.root, files.geometry) : null;
    const still = () => gameArt() === art && map() === m;
    if (!files.views) { set({ glb, views: null, image: () => "" }); return; }
    const viewsFile = files.views;
    void loadViews(url(art.root, viewsFile)).then((views) => {
      if (!still()) return;
      const image = (id: string) => url(art.root, viewImageFile(viewsFile, views?.views.find((v) => v.id === id) ?? { id }));
      set(views || glb ? { glb, views, image } : null);
    });
  });
  return geo;
}
