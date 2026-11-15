import { For, Show } from "solid-js";
import type { GameState, Kill } from "../state/reducer";
import { colourFor } from "../lib/game";
import { killVerb } from "../lib/format";
import { VerifyMark } from "./bits";

export default function KillFeed(props: { game: GameState; kills: Kill[]; limit?: number }) {
  const shown = () => props.kills.slice(-(props.limit ?? 12)).reverse();
  const who = (i: number | null) => (i === null ? null : props.game.players[i]);
  const Name = (p: { i: number | null }) => {
    const pl = who(p.i);
    return (
      <span class="kf-name" style={{ color: pl ? colourFor(props.game, pl.team, pl.index) : undefined }}>
        {pl?.name ?? (p.i === null ? "the world" : `player ${p.i}`)}
      </span>
    );
  };
  return (
    <ol class="killfeed" aria-live="polite">
      <For each={shown()} fallback={<li class="killfeed__empty">No knockouts yet. Everyone's being nice.</li>}>
        {(k) => (
          <li class={`kf kf--${k.kind}`}>
            <Show when={k.kind === "kill" || k.kind === "betrayal"} fallback={<><Name i={k.victim} /> <span class="kf-verb">{killVerb(k.kind, k.position)}</span></>}>
              <Name i={k.killer} /> <span class="kf-verb">{killVerb(k.kind, k.position)}</span> <Name i={k.victim} />
            </Show>
            <Show when={k.weapon}><span class="kf-weapon">{k.weapon}</span></Show>
            <Show when={k.position > 0}><VerifyMark position={k.position} compact /></Show>
          </li>
        )}
      </For>
    </ol>
  );
}
