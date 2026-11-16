import { For, Show } from "solid-js";
import type { GameState, Kill } from "../state/reducer";
import { colourFor } from "../lib/game";
import { killVerb } from "../lib/format";
import { VerifyMark } from "./bits";

export default function KillFeed(props: { game: GameState; kills: Kill[]; limit?: number }) {
  const shown = () => props.kills.slice(-(props.limit ?? 12)).reverse();
  const who = (i: number | null) => (i === null ? null : props.game.players[i]);
  // The team colour rides a custom property, so the stats side can restyle
  // the names (plain link colour, a small team square) while the dashboard
  // keeps its coloured names.
  const Name = (p: { i: number | null }) => {
    const pl = who(p.i);
    const colour = pl ? colourFor(props.game, pl.team, pl.index) : null;
    return (
      <span class="kf-name" style={{ "--kf-colour": colour ?? undefined }}>
        <Show when={colour}><span class="kf-team" style={{ background: colour ?? undefined }} aria-hidden="true" /></Show>
        {pl?.name ?? (p.i === null ? "the world" : `player ${p.i}`)}
      </span>
    );
  };
  // One signature mark for the whole feed (shown in the stats-side header),
  // standing in for the per-row marks that side hides.
  const signed = () => shown().find((k) => k.position > 0);
  return (
    <>
      <Show when={signed()}>{(k) => <div class="killfeed__head"><VerifyMark position={k().position} compact /></div>}</Show>
      <ol class="killfeed" aria-live="polite">
        <For each={shown()} fallback={<li class="killfeed__empty">No knockouts yet. Everyone's being nice.</li>}>
          {(k) => (
            <li class={`kf kf--${k.kind}`}>
              <Show
                when={(k.kind === "kill" && k.killer !== null) || k.kind === "betrayal"}
                fallback={<><Name i={k.victim} /> <span class="kf-verb">{k.kind === "kill" ? "was killed" : killVerb(k.kind, k.position)}</span></>}
              >
                <Name i={k.killer} /> <span class="kf-verb">{killVerb(k.kind, k.position)}</span> <Name i={k.victim} />
              </Show>
              <Show when={k.weapon}><span class="kf-weapon">{k.weapon}</span></Show>
              <Show when={k.position > 0}><VerifyMark position={k.position} compact /></Show>
            </li>
          )}
        </For>
      </ol>
    </>
  );
}
