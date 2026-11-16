// Player emblems. With the server's game art (state/gameart.ts) an emblem is
// two mask layers coloured on a canvas; otherwise it is one of the art
// pack's original two-colour symbols. The pick is deterministic from the
// seed (an XUID or gamertag) until telemetry carries a player's real
// appearance: `appearance` (or a ready `emblem`) when the caller has it,
// else the latest one the player's games carried, looked up by XUID.

import { createEffect, createSignal, onCleanup } from "solid-js";
import { emblemArt } from "../lib/art";
import { emblemChoice, emblemFromAppearance, type Appearance, type EmblemSpec } from "../lib/gameart";
import { emblemLayers, emblemUrl } from "../state/gameart";
import { appearanceOf } from "../state/appearance";

/** A stable index for a seed, spread better than a plain sum. */
function pick(seed: string): number {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i++) h = Math.imul(h ^ seed.charCodeAt(i), 16777619);
  return (h >>> 0) % 24;
}

export default function Emblem(props: {
  seed: string; size?: number; class?: string; title?: string;
  /** The player's real emblem (layers and colours), once it is known;
   *  `emblemFromIndices` turns profile colour indices into one. */
  emblem?: Partial<EmblemSpec>;
  /** The player's appearance as telemetry reports it (emblem layers and
   *  profile colour indices); over the seed-derived pick when present. */
  appearance?: Appearance | null;
}) {
  const size = () => props.size ?? 48;
  const pack = () => emblemArt(pick(props.seed), size());
  const [game, setGame] = createSignal<string | null>(null);
  createEffect(() => {
    const l = emblemLayers();
    if (!l) { setGame(null); return; }
    const spec: EmblemSpec = {
      ...emblemFromAppearance(l.palette, props.appearance ?? appearanceOf(props.seed), emblemChoice(props.seed, l.fg, l.bg, l.palette)),
      ...props.emblem,
    };
    let current = true;
    onCleanup(() => { current = false; });
    void emblemUrl(l.a, spec).then((url) => { if (current) setGame(url); });
  });
  return (
    <img
      class={`emblem ${props.class ?? ""}`}
      classList={{ "emblem--game": !!game() }}
      src={game() ?? pack().src}
      srcset={game() ? undefined : pack().srcset}
      width={size()}
      height={size()}
      alt={props.title ?? ""}
      decoding="async"
      draggable={false}
    />
  );
}
