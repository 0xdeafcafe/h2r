import { For, Show } from "solid-js";
import { A } from "@solidjs/router";
import type { GameState, Standing, TeamStanding } from "../state/reducer";
import { standings, teamStandings } from "../state/reducer";
import { colourFor, teamName } from "../lib/game";
import { STATS_ROOT } from "../routes";
import { BotTag } from "./bits";

interface ScoreboardActions {
  onHover?: (i: number | null) => void;
  onPick?: (i: number) => void;
  picked?: number | null;
}

function Row(props: ScoreboardActions & { g: GameState; s: Standing; rank: number }) {
  return (
    <tr
      class="score-row"
      classList={{ "score-row--left": props.s.left, "score-row--picked": props.picked === props.s.index }}
      style={{ "--row": colourFor(props.g, props.s.team, props.s.index) }}
      onMouseEnter={() => props.onHover?.(props.s.index)}
      onMouseLeave={() => props.onHover?.(null)}
      onFocusIn={() => props.onHover?.(props.s.index)}
      onFocusOut={() => props.onHover?.(null)}
    >
      <td class="score-rank">{props.rank}</td>
      <td class="score-name">
        <span class="swatch" style={{ background: colourFor(props.g, props.s.team, props.s.index) }} />
        {/* A bot with an xuid is a server bot account and has a page (bots.md §6). */}
        <Show when={props.onPick} fallback={
          <Show when={props.s.xuid} fallback={props.s.name}>
            {(x) => <A href={`${STATS_ROOT}/players/${encodeURIComponent(props.s.name)}`} title={props.s.game_name ? `${x()} · in game as ${props.s.game_name}` : x()}>{props.s.name}</A>}
          </Show>
        }>
          <button type="button" class="score-pov" aria-pressed={props.picked === props.s.index} onClick={() => props.onPick?.(props.s.index)} title={`Watch ${props.s.name}`}>
            {props.s.name}
          </button>
        </Show>
        <Show when={props.s.bot}><BotTag /></Show>
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
      <tr><th scope="col"><span class="sr-only">Place</span></th><th scope="col">Players</th><th scope="col" class="num">Score</th><th scope="col" class="num" title="Kills">K</th><th scope="col" class="num" title="Deaths">D</th><th scope="col" class="num" title="Assists">A</th></tr>
    </thead>
  );
}

/** Live scoreboard. `game` may be a replay snapshot shaped like GameState. */
export default function Scoreboard(props: ScoreboardActions & { game: GameState }) {
  return (
    <div class="scoreboard">
      <Show
        when={props.game.team_play}
        fallback={
          <table class="score-table" aria-label="Free-for-all scoreboard">
            <Head />
            <tbody>
              <For each={standings(props.game)}>{(s, i) => <Row g={props.game} s={s} rank={i() + 1} onHover={props.onHover} onPick={props.onPick} picked={props.picked} />}</For>
            </tbody>
          </table>
        }
      >
        <For each={teamStandings(props.game)}>
          {(t: TeamStanding) => (
            <section class="team-block" style={{ "--team-colour": colourFor(props.game, t.team, 0) }}>
              <header class="team-head">
                <span>{teamName(t.team)} Team</span>
                <strong>{t.score}</strong>
              </header>
              <table class="score-table" aria-label={`${teamName(t.team)} Team scoreboard`}>
                <Head />
                <tbody>
                  <For each={t.players}>{(s, i) => <Row g={props.game} s={s} rank={i() + 1} onHover={props.onHover} onPick={props.onPick} picked={props.picked} />}</For>
                </tbody>
              </table>
            </section>
          )}
        </For>
      </Show>
    </div>
  );
}
