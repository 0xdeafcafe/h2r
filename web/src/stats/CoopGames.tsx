// Finished co-op campaign games (/v1/coop/games): who played, the level,
// the difficulty and when. A grid in the stats' house style, for the games
// page's campaign tab and a player's history.

import { For, Show, type Accessor } from "solid-js";
import { A } from "@solidjs/router";
import { useCoopGames } from "../state/sessions";
import { coopDifficulty, coopLevel, coopMinutes, type CoopGame } from "../lib/coop";
import { playerHref } from "./Layout";
import { usDateTime } from "./fmt";

function Names(props: { game: CoopGame }) {
  return (
    <For each={props.game.players}>
      {(p, i) => <>{i() > 0 ? ", " : ""}<Show when={p.gamertag} fallback={<span class="s-dim">guest</span>}>{(t) => <A href={playerHref(t())}>{t()}</A>}</Show></>}
    </For>
  );
}

export function CoopTable(props: { games: CoopGame[]; empty: string }) {
  return (
    <table class="s-grid">
      <caption class="sr-only">Finished co-op campaign games</caption>
      <thead><tr><th>Level</th><th>Players</th><th class="hide-s">Difficulty</th><th class="num">Played</th><th class="num hide-s">Length</th></tr></thead>
      <tbody>
        <For each={props.games} fallback={<tr><td colspan="5" class="s-dim">{props.empty}</td></tr>}>
          {(g) => (
            <tr>
              <td>{coopLevel(g)}</td>
              <td><Names game={g} /></td>
              <td class="hide-s">{coopDifficulty(g)}</td>
              <td class="num">{usDateTime(g.ended)}</td>
              <td class="num hide-s">{coopMinutes(g) ?? "–"} min</td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}

/** The co-op games of one player (a gamertag), or of everyone. */
export default function CoopGames(props: { player?: Accessor<string | undefined> }) {
  const feed = useCoopGames(props.player);
  const empty = () => feed.status() === "loading" ? "Looking for co-op games."
    : feed.status() === "unsupported" ? "This server doesn't list co-op games."
    : feed.status() === "error" ? "The server didn't answer."
    : "No co-op campaign games on record yet.";
  return <div class="s-grid-wrap"><CoopTable games={feed.items()} empty={empty()} /></div>;
}
