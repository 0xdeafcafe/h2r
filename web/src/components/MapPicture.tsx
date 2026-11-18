// A map's (or campaign level's) picture: for a map package (a game or
// session with a map_id), the package's own image first; then the base
// map's from the server's game art: its preview, else the straight-down
// view drawn from its geometry (`views/top.png`), else nothing, and the
// caller shows its placeholder.

import { Show, createSignal, type Accessor, type JSX } from "solid-js";
import { mapKey } from "../lib/gameart";
import { pictureChoice } from "../lib/maps";
import { artStem, gameImage, mapTitle } from "../state/gameart";
import { gameMapTitle, packageImage } from "../state/maps";
import { geometryFor } from "../map/geometry";
import { topView } from "../map/views";

/** The URL of a map's picture, or null. Reactive; the geometry's views are
 *  only asked for when there is no package image or preview. */
export function mapPicture(map: Accessor<string | null | undefined>, mapId: Accessor<string | null | undefined> = () => null): Accessor<string | null> {
  const pkg = () => packageImage(mapId());
  const preview = () => {
    const m = map();
    return m ? gameImage(mapKey(artStem(m)))?.url ?? null : null;
  };
  const geo = geometryFor(() => (pkg() || preview() ? undefined : map() ?? undefined));
  const base = () => {
    const p = preview();
    if (p) return p;
    const g = geo();
    const top = topView(g?.views);
    return g && top ? g.image(top.id) : null;
  };
  return () => pictureChoice(pkg(), base())?.src ?? null;
}

export default function MapPicture(props: { map: string; mapId?: string | null; class?: string; fallback?: JSX.Element }) {
  const picture = mapPicture(() => props.map, () => props.mapId);
  // One that fails to load is shown as none, never as a broken image.
  const [broken, setBroken] = createSignal<string | null>(null);
  const url = () => { const u = picture(); return u && u !== broken() ? u : null; };
  const alt = () => (props.mapId ? gameMapTitle({ map: props.map, map_id: props.mapId }) : mapTitle(props.map));
  return (
    <Show when={url()} fallback={props.fallback ?? null}>
      {(u) => <img class={props.class} src={u()} alt={alt()} decoding="async" draggable={false} onError={() => setBroken(u())} />}
    </Show>
  );
}
