// A game type's icon from the server's game art, by variant name. Renders
// nothing without game art (the art pack has no game-type icons).

import { Show } from "solid-js";
import { gametypeKey } from "../lib/gameart";
import { gameImage } from "../state/gameart";

export default function GametypeIcon(props: { variant: string | null | undefined; size?: number }) {
  const size = () => props.size ?? 16;
  return (
    <Show when={gameImage(gametypeKey(props.variant))}>
      {(g) => <span class="gametype-icon" style={{ width: `${size()}px`, height: `${size()}px` }}><img src={g().url} alt="" decoding="async" draggable={false} /></span>}
    </Show>
  );
}
