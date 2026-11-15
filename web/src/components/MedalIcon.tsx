// Medal badges: the art pack's original medals, one per derived medal id.

import type { MedalId } from "../state/derive";
import { MEDAL_BY_ID } from "../state/derive";
import { medalArt } from "../lib/art";

export default function MedalIcon(props: { id: MedalId; size?: number; dim?: boolean }) {
  const def = () => MEDAL_BY_ID[props.id];
  const size = () => props.size ?? 28;
  const art = () => medalArt(props.id, size());
  return (
    <img
      class="medal"
      classList={{ "medal--dim": !!props.dim }}
      src={art().src}
      srcset={art().srcset}
      width={size()}
      height={size()}
      alt={def().name}
      title={`${def().name}: ${def().blurb} (derived from game events)`}
      decoding="async"
      draggable={false}
    />
  );
}
