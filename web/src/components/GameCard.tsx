import { For, Show, createMemo } from "solid-js";
import { A } from "@solidjs/router";
import type { GameState } from "../state/reducer";
import { standings, teamStandings } from "../state/reducer";
import MapCanvas from "./MapCanvas";
import { colourFor, elapsedSeconds, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { clock, titleCase, ago } from "../lib/format";
import { now } from "../lib/now";
import { outlineFor } from "../state/outlines";

export default function GameCard(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  const frame = liveFrameSource(props.game.game_id, roster);
  return (
    <A href={`/games/${props.game.game_id}`} class="game-card" classList={{ "game-card--live": props.game.status === "live" }}>
      <div class="game-card__map">
        <Show when={props.game.status === "live"} fallback={<div class="game-card__ended">ended {ago(props.game.ended_at, now())}</div>}>
          <MapCanvas mini frame={frame} roster={roster} outline={outline} label={`Live map of ${props.game.map}`} />
          <span class="live-pill">live</span>
        </Show>
      </div>
      <div class="game-card__body">
        <div class="game-card__title">
          <strong>{titleCase(props.game.map)}</strong>
          <span class="game-card__clock">{clock(elapsedSeconds(props.game, now()))}</span>
        </div>
        <div class="game-card__meta">
          {titleCase(props.game.variant ?? props.game.mode)} · {Object.keys(props.game.players).length} players
        </div>
        <ul class="mini-scores">
          <Show
            when={props.game.team_play}
            fallback={<For each={standings(props.game).slice(0, 3)}>{(s) => (
              <li><span class="swatch" style={{ background: colourFor(props.game, s.team, s.index) }} />{s.name}<b>{s.score.score}</b></li>
            )}</For>}
          >
            <For each={teamStandings(props.game).slice(0, 4)}>{(t) => (
              <li><span class="swatch" style={{ background: colourFor(props.game, t.team, 0) }} />{teamName(t.team)}<b>{t.score}</b></li>
            )}</For>
          </Show>
        </ul>
      </div>
    </A>
  );
}
