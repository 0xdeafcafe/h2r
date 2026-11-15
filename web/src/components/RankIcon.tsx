// Rank insignia for the derived rank ladder: the game's level icon when the
// server has game art and `exp` is given, else the art pack's badge.

import { Show } from "solid-js";
import { RANKS } from "../state/derive";
import { rankArt } from "../lib/art";
import { levelForExp, rankKey } from "../lib/gameart";
import { gameImage } from "../state/gameart";

export default function RankIcon(props: { rank: number; exp?: number; size?: number }) {
  const size = () => props.size ?? 32;
  const name = () => RANKS[props.rank]?.[1] ?? "";
  const level = () => (props.exp === undefined ? null : levelForExp(props.exp));
  const game = () => (level() === null ? null : gameImage(rankKey(level()!)));
  return (
    <Show
      when={game()}
      fallback={
        <Show when={rankArt(name(), size())}>
          {(art) => <img class="rank-icon" src={art().src} srcset={art().srcset} width={size()} height={size()} alt={name()} title={name()} decoding="async" draggable={false} />}
        </Show>
      }
    >
      {(g) => <img class="rank-icon rank-icon--game" src={g().url} width={size()} height={size()} alt={`${name()}, level ${level()}`} title={`${name()} (level ${level()})`} decoding="async" draggable={false} />}
    </Show>
  );
}
