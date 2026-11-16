// A map's (or campaign level's) picture from the server's game art: its
// preview, else the straight-down view drawn from its geometry
// (`views/top.png`), else nothing, and the caller shows its placeholder.

import { Show, type Accessor, type JSX } from "solid-js";
import { mapKey } from "../lib/gameart";
import { gameImage, mapTitle } from "../state/gameart";
import { geometryFor } from "../map/geometry";
import { topView } from "../map/views";

/** The URL of a map's picture, or null. Reactive; the geometry's views are
 *  only asked for when there is no preview. */
export function mapPicture(map: Accessor<string | null | undefined>): Accessor<string | null> {
  const preview = () => {
    const m = map();
    return m ? gameImage(mapKey(m))?.url ?? null : null;
  };
  const geo = geometryFor(() => (preview() ? undefined : map() ?? undefined));
  return () => {
    const p = preview();
    if (p) return p;
    const g = geo();
    const top = topView(g?.views);
    return g && top ? g.image(top.id) : null;
  };
}

export default function MapPicture(props: { map: string; class?: string; fallback?: JSX.Element }) {
  const url = mapPicture(() => props.map);
  return (
    <Show when={url()} fallback={props.fallback ?? null}>
      {(u) => <img class={props.class} src={u()} alt={`${mapTitle(props.map)}`} decoding="async" draggable={false} />}
    </Show>
  );
}
