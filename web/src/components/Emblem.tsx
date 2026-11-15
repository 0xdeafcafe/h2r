// Player emblems. With the server's game art (state/gameart.ts) an emblem is
// two mask layers coloured on a canvas; otherwise it is one of the art
// pack's original two-colour symbols. Either way the pick is deterministic
// from the seed (an XUID or gamertag) until telemetry carries a player's
// real emblem, which `emblem` is for.

import { createEffect, createSignal, onCleanup } from "solid-js";
import { emblemArt } from "../lib/art";
import { emblemChoice, type EmblemSpec } from "../lib/gameart";
import { emblemLayers, emblemUrl } from "../state/gameart";

/** A stable index for a seed, spread better than a plain sum. */
function pick(seed: string): number {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i++) h = Math.imul(h ^ seed.charCodeAt(i), 16777619);
  return (h >>> 0) % 24;
}

export default function Emblem(props: {
  seed: string; size?: number; class?: string; title?: string;
  /** The player's real emblem (layers and colours), once it is known. */
  emblem?: Partial<EmblemSpec>;
}) {
  const size = () => props.size ?? 48;
  const pack = () => emblemArt(pick(props.seed), size());
  const [game, setGame] = createSignal<string | null>(null);
  createEffect(() => {
    const l = emblemLayers();
    if (!l) { setGame(null); return; }
    const spec: EmblemSpec = { ...emblemChoice(props.seed, l.fg, l.bg), ...props.emblem };
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
