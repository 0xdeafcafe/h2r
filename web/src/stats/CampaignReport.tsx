// The report under a campaign game, in place of the carnage report: who
// played, how often each died and, when the game reports them, how many of
// the level's AI they knocked out. No places, scores or team colours.

import { For, Show, createMemo } from "solid-js";
import { A } from "@solidjs/router";
import type { GameState } from "../state/reducer";
import { OUTCOME_WORDS, campaignOutcome, deathsOf } from "../state/derive";
import { difficultyName } from "../state/gametype";
import { gameMapTitle } from "../state/maps";
import Emblem from "../components/Emblem";
import { gameSeconds } from "../lib/game";
import { now } from "../lib/now";
import { Derived, Title, playerHref } from "./Layout";
import { gameTitle } from "./Side";
import { hms } from "./fmt";

export interface CampaignPlayerRow {
  index: number;
  name: string;
  game_name: string | null;
  xuid: string | null;
  appearance: GameState["players"][number]["appearance"];
  left: boolean;
  deaths: number;
  /** Null when the game reported no kills for anyone. */
  kills: number | null;
}

/** A campaign game's players in slot order, with deaths and kills. */
export function campaignPlayers(g: GameState): CampaignPlayerRow[] {
  const anyKills = Object.values(g.scores).some((s) => s.kills > 0);
  return Object.values(g.players)
    .sort((a, b) => a.index - b.index)
    .map((p) => ({
      index: p.index, name: p.name, game_name: p.game_name, xuid: p.xuid ?? null, appearance: p.appearance, left: p.left,
      deaths: deathsOf(g, [p.index]),
      kills: anyKills ? g.scores[p.index]?.kills ?? 0 : null,
    }));
}

export function PlayerName(props: { r: CampaignPlayerRow }) {
  return (
    <span class="who">
      <Show when={props.r.xuid} fallback={<><Emblem seed={props.r.name} appearance={props.r.appearance} size={19} /><span>{props.r.name}</span></>}>
        {(x) => <><Emblem seed={x()} appearance={props.r.appearance} size={19} /><A href={`${playerHref(props.r.name)}/campaign`}>{props.r.name}</A></>}
      </Show>
      <Show when={props.r.game_name}>{(n) => <small class="who-game-name" title="Name in the game">{n()}</small>}</Show>
      <Show when={props.r.left}><small>(left)</small></Show>
    </span>
  );
}

export default function CampaignReport(props: { game: GameState }) {
  const rows = createMemo(() => campaignPlayers(props.game));
  const kills = () => rows().some((r) => r.kills !== null);
  const total = (f: (r: CampaignPlayerRow) => number) => rows().reduce((n, r) => n + f(r), 0);
  const ended = () => props.game.status === "ended";
  return (
    <section class="s-game-report s-campaign-report" aria-label="Mission report">
      <Title right={<Derived note="Deaths come from the game's scoreboard or its death events, whichever counts more. Kills are of the level's AI, when the game reports them." />}>
        {ended() ? "Mission Report" : "Mission Report So Far"}
      </Title>
      <div class="s-carnage">
        <p class="s-report-note">
          {gameTitle(props.game)} on {gameMapTitle(props.game)}
          <span>{[difficultyName(props.game.difficulty), OUTCOME_WORDS[campaignOutcome(props.game)], hms(gameSeconds(props.game, now()))].filter(Boolean).join(" · ")}</span>
        </p>
        <table class="s-stats s-campaign-table" aria-label="Players">
          <colgroup><col /><col style={{ width: "90px" }} /><col style={{ width: "90px" }} /></colgroup>
          <thead><tr><th>Players</th><th>Deaths</th><th>{kills() ? "AI Kills" : ""}</th></tr></thead>
          <tbody>
            <For each={rows()}>
              {(r) => (
                <tr classList={{ gone: r.left }}>
                  <td><PlayerName r={r} /></td>
                  <td>{r.deaths}</td>
                  <td>{r.kills ?? ""}</td>
                </tr>
              )}
            </For>
            <Show when={rows().length > 1}>
              <tr class="s-campaign-total"><td>Everyone</td><td>{total((r) => r.deaths)}</td><td>{kills() ? total((r) => r.kills ?? 0) : ""}</td></tr>
            </Show>
          </tbody>
        </table>
      </div>
    </section>
  );
}
