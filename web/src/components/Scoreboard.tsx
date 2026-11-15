import { For, Show } from "solid-js";
import { A } from "@solidjs/router";
import type { GameState, Standing, TeamStanding } from "../state/reducer";
import { standings, teamStandings } from "../state/reducer";
import { colourFor, teamName } from "../lib/game";

function Row(props: { g: GameState; s: Standing; rank: number; onHover?: (i: number | null) => void }) {
  return (
    <tr
      class="score-row"
      classList={{ "score-row--left": props.s.left }}
      onMouseEnter={() => props.onHover?.(props.s.index)}
      onMouseLeave={() => props.onHover?.(null)}
    >
      <td class="score-rank">{props.rank}</td>
      <td class="score-name">
        <span class="swatch" style={{ background: colourFor(props.g, props.s.team, props.s.index) }} />
        <Show when={props.s.xuid} fallback={props.s.name}>
          {(x) => <A href={`/stats/players/${encodeURIComponent(props.s.name)}`} title={x()}>{props.s.name}</A>}
        </Show>
        <Show when={props.s.left}> <small>(left)</small></Show>
      </td>
      <td class="num score-score">{props.s.score.score}</td>
      <td class="num">{props.s.score.kills}</td>
      <td class="num">{props.s.score.deaths}</td>
      <td class="num">{props.s.score.assists}</td>
    </tr>
  );
}

function Head() {
  return (
    <thead>
      <tr><th /><th>player</th><th class="num">score</th><th class="num" title="kills">K</th><th class="num" title="deaths">D</th><th class="num" title="assists">A</th></tr>
    </thead>
  );
}

/** Live scoreboard. `game` may be a replay snapshot shaped like GameState. */
export default function Scoreboard(props: { game: GameState; onHover?: (i: number | null) => void }) {
  return (
    <div class="scoreboard">
      <Show
        when={props.game.team_play}
        fallback={
          <table class="score-table">
            <Head />
            <tbody>
              <For each={standings(props.game)}>{(s, i) => <Row g={props.game} s={s} rank={i() + 1} onHover={props.onHover} />}</For>
            </tbody>
          </table>
        }
      >
        <For each={teamStandings(props.game)}>
          {(t: TeamStanding) => (
            <section class="team-block" style={{ "--team-colour": colourFor(props.game, t.team, 0) }}>
              <header class="team-head">
                <span>{teamName(t.team)} team</span>
                <strong>{t.score}</strong>
              </header>
              <table class="score-table">
                <tbody>
                  <For each={t.players}>{(s, i) => <Row g={props.game} s={s} rank={i() + 1} onHover={props.onHover} />}</For>
                </tbody>
              </table>
            </section>
          )}
        </For>
      </Show>
    </div>
  );
}
