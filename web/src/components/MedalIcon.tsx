// Medal badges, one per medal id the server counts: the game's own medal
// when the server has game art and the medal has a counterpart there, else
// the art pack's original, else (an id the pack has no file for) the
// medal's name in a badge of the same size. An id the site doesn't list (a
// newer server's medal) shows as its id.

import { Show } from "solid-js";
import { medalDef, medalName } from "../state/derive";
import { medalArt } from "../lib/art";
import { medalKey } from "../lib/gameart";
import { gameImage } from "../state/gameart";

export default function MedalIcon(props: { id: string; size?: number; dim?: boolean }) {
  const name = () => medalName(props.id);
  const size = () => props.size ?? 28;
  const art = () => medalArt(props.id, size());
  const game = () => gameImage(medalKey(props.id));
  const title = () => { const d = medalDef(props.id); return d ? `${d.name}: ${d.blurb}` : name(); };
  return (
    <Show
      when={game() || art()}
      fallback={
        <span
          class="medal medal--name"
          classList={{ "medal--dim": !!props.dim }}
          style={{ width: `${size()}px`, height: `${size()}px`, "font-size": `${Math.max(7, Math.round(size() / 4.5))}px` }}
          role="img"
          aria-label={name()}
          title={title()}
        >
          {name()}
        </span>
      }
    >
      <img
        class="medal"
        classList={{ "medal--dim": !!props.dim, "medal--game": !!game() }}
        src={game()?.url ?? art()?.src}
        srcset={game() ? undefined : art()?.srcset}
        width={size()}
        height={size()}
        alt={name()}
        title={title()}
        decoding="async"
        draggable={false}
      />
    </Show>
  );
}
