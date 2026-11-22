import { For, Show, createMemo, onMount } from "solid-js";
import type { GameState } from "../state/reducer";
import { colourFor } from "../lib/game";
import { damageSourceName, killVerb } from "../lib/format";
import { VerifyMark } from "./bits";
import { killFeedRows, type FeedKill } from "./killFeedRows";

export default function KillFeed(props: { game: GameState; kills: FeedKill[]; limit?: number }) {
  const shown = createMemo(() => killFeedRows(props.kills, props.limit ?? 12));
  const byKey = createMemo(() => new Map(shown().map(({ key, kill }) => [key, kill])));
  const seen = new Set<string>();
  let mounted = false;
  onMount(() => { mounted = true; });

  // The dashboard and stats site style the name and team square separately.
  const Name = (p: { i: number | null }) => {
    const player = () => p.i === null ? null : props.game.players[p.i];
    const colour = () => {
      const pl = player();
      return pl ? colourFor(props.game, pl.team, pl.index) : undefined;
    };
    return (
      <span class="kf-name" style={{ "--kf-colour": colour() }}>
        <Show when={colour()}><span class="kf-team" style={{ background: colour() }} aria-hidden="true" /></Show>
        {player()?.name ?? (p.i === null ? "the world" : `player ${p.i}`)}
      </span>
    );
  };
  const signed = () => shown().find(({ kill }) => kill.position > 0)?.kill.position;
  return (
    <>
      <Show when={signed()}>{(position) => <div class="killfeed__head"><VerifyMark position={position()} compact /></div>}</Show>
      <ol class="killfeed" aria-live="polite" aria-relevant="additions">
        <For each={shown().map(({ key }) => key)} fallback={<li class="killfeed__empty">No knockouts yet. Everyone's being nice.</li>}>
          {(key, index) => {
            // Solid keys <For> by item identity. Keep the primitive key as
            // the item and look up its data independently as the clock moves.
            const kill = createMemo((previous: FeedKill) => byKey().get(key) ?? previous, byKey().get(key)!);
            const animate = mounted && index() === 0 && !seen.has(key);
            seen.add(key);
            const verb = () => killVerb(kill().kind, kill().position);
            return (
              <li class={`kf kf--${kill().kind}`} classList={{ "kf--new": animate }}>
                <Show
                  when={(kill().kind === "kill" && kill().killer !== null) || kill().kind === "betrayal"}
                  fallback={<><Name i={kill().victim} />{" "}<span class="kf-verb">{kill().kind === "kill" ? "was killed" : verb()}</span></>}
                >
                  <Name i={kill().killer} />{" "}<span class="kf-verb">{verb()}</span>{" "}<Name i={kill().victim} />
                </Show>
                <Show when={kill().weapon || kill().headshot}>
                  <span class="kf-weapon">
                    {[damageSourceName(kill().weapon ?? ""), kill().headshot && "Headshot"].filter(Boolean).join(" · ")}
                  </span>
                </Show>
                <Show when={kill().position > 0}><VerifyMark position={kill().position} compact /></Show>
              </li>
            );
          }}
        </For>
      </ol>
    </>
  );
}
