// Medal badges, one per derived medal id: the game's own medal when the
// server has game art and the medal has a counterpart there, else the art
// pack's original.

import type { MedalId } from "../state/derive";
import { MEDAL_BY_ID } from "../state/derive";
import { medalArt } from "../lib/art";
import { medalKey } from "../lib/gameart";
import { gameImage } from "../state/gameart";

export default function MedalIcon(props: { id: MedalId; size?: number; dim?: boolean }) {
  const def = () => MEDAL_BY_ID[props.id];
  const size = () => props.size ?? 28;
  const art = () => medalArt(props.id, size());
  const game = () => gameImage(medalKey(props.id));
  return (
    <img
      class="medal"
      classList={{ "medal--dim": !!props.dim, "medal--game": !!game() }}
      src={game()?.url ?? art().src}
      srcset={game() ? undefined : art().srcset}
      width={size()}
      height={size()}
      alt={def().name}
      title={`${def().name}: ${def().blurb} (derived from game events)`}
      decoding="async"
      draggable={false}
    />
  );
}
