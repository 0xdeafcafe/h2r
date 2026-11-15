// Rank insignia for the derived rank ladder, from the art pack's badges.

import { Show } from "solid-js";
import { RANKS } from "../state/derive";
import { rankArt } from "../lib/art";

export default function RankIcon(props: { rank: number; size?: number }) {
  const size = () => props.size ?? 32;
  const name = () => RANKS[props.rank]?.[1] ?? "";
  return (
    <Show when={rankArt(name(), size())}>
      {(art) => <img class="rank-icon" src={art().src} srcset={art().srcset} width={size()} height={size()} alt={name()} title={name()} decoding="async" draggable={false} />}
    </Show>
  );
}
