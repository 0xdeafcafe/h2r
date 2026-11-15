// Player emblems: one of the art pack's original two-colour symbols, picked
// deterministically from the XUID (or gamertag).

import { emblemArt } from "../lib/art";

/** A stable index for a seed, spread better than a plain sum. */
function pick(seed: string): number {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i++) h = Math.imul(h ^ seed.charCodeAt(i), 16777619);
  return (h >>> 0) % 24;
}

export default function Emblem(props: { seed: string; size?: number; class?: string; title?: string }) {
  const size = () => props.size ?? 48;
  const art = () => emblemArt(pick(props.seed), size());
  return (
    <img
      class={`emblem ${props.class ?? ""}`}
      src={art().src}
      srcset={art().srcset}
      width={size()}
      height={size()}
      alt={props.title ?? ""}
      decoding="async"
      draggable={false}
    />
  );
}
