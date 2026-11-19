// A player emblem requests its composed image immediately. The art pack's
// deterministic symbol is available for servers without the requested art.

import { createSignal } from "solid-js";
import { emblemArt, packEmblem } from "../lib/art";
import { emblemPath } from "../lib/emblem";
import type { Appearance, EmblemSpec } from "../lib/gameart";
import { appearanceOf } from "../state/appearance";
import { live } from "../state/live";

export default function Emblem(props: {
  seed: string; size?: number; class?: string; title?: string;
  /** Explicit emblem layers or colours supplied by the caller. */
  emblem?: Partial<EmblemSpec>;
  /** Appearance for this game. Omit to use the player's latest appearance. */
  appearance?: Appearance | null;
}) {
  const size = () => props.size ?? 48;
  const pack = () => emblemArt(packEmblem(props.seed), size());
  const [failed, setFailed] = createSignal<string>();
  const src = () => live.server().base + emblemPath(
    props.seed,
    props.appearance === undefined ? appearanceOf(props.seed) : props.appearance,
    props.emblem,
  );
  const game = () => failed() !== src();
  return (
    <img
      class={`emblem ${props.class ?? ""}`}
      classList={{ "emblem--game": game() }}
      src={game() ? src() : pack().src}
      srcset={game() ? undefined : pack().srcset}
      onError={(event) => {
        if (game() && event.currentTarget.getAttribute("src") === src()) setFailed(src());
      }}
      width={size()}
      height={size()}
      alt={props.title ?? ""}
      decoding="async"
      draggable={false}
    />
  );
}
