// The carnage report table under a game: places, scores, sprees, medals and
// weapons for every player, derived from the game's events.

import { For, Show, createMemo } from "solid-js";
import { A } from "@solidjs/router";
import type { GameState } from "../state/reducer";
import { standings } from "../state/reducer";
import { bestSprees, countMedals, gameMedals, ordinal, places, weaponBreakdown } from "../state/derive";
import Emblem from "../components/Emblem";
import MedalIcon from "../components/MedalIcon";
import { colourFor, teamName } from "../lib/game";
import { titleCase } from "../lib/format";
import { Derived, Panel } from "./Layout";

export default function Carnage(props: { game: GameState }) {
  const rows = createMemo(() => {
    const g = props.game;
    const pl = places(g);
    const sprees = bestSprees(g.kills);
    const medals = gameMedals(g.kills);
    return standings(g)
      .map((s) => ({ s, place: pl.get(s.index) ?? 0, spree: sprees.get(s.index) ?? 0, medals: [...countMedals(medals, new Set([s.index])).entries()] }))
      .sort((a, b) => a.place - b.place || b.s.score.score - a.s.score.score || a.s.index - b.s.index);
  });
  const weapons = createMemo(() => weaponBreakdown(props.game.kills));
  const max = () => Math.max(1, ...weapons().map((w) => w.kills));
  return (
    <div class="s-carnage">
      <Panel title={props.game.status === "ended" ? "Carnage Report" : "Carnage So Far"} right={<Derived note="Places, sprees and medals are derived in your browser from the game's events. Snapshots carry each game's last 50 knockouts." />}>
        <div class="s-scroll" tabindex="0" role="region" aria-label="Carnage report">
          <table class="s-table s-table--carnage">
            <thead>
              <tr><th>Place</th><th>Player</th><th>Team</th><th class="num">Score</th><th class="num">Kills</th><th class="num">Deaths</th><th class="num">Assists</th><th class="num">K/D</th><th class="num">Best Spree</th><th>Medals</th></tr>
            </thead>
            <tbody>
              <For each={rows()}>
                {(r) => (
                  <tr classList={{ left: r.s.left }}>
                    <td>{props.game.status === "ended" ? ordinal(r.place) : r.place}</td>
                    <td class="s-player-cell">
                      <span class="swatch" style={{ background: colourFor(props.game, r.s.team, r.s.index) }} />
                      <Show when={r.s.xuid} fallback={<span>{r.s.name}</span>}>
                        {(x) => <><Emblem seed={x()} size={18} /><A href={`/stats/players/${encodeURIComponent(r.s.name)}`}>{r.s.name}</A></>}
                      </Show>
                      <Show when={r.s.left}> <small class="s-dim">(left)</small></Show>
                    </td>
                    <td>{props.game.team_play ? teamName(r.s.team) : "–"}</td>
                    <td class="num">{r.s.score.score}</td>
                    <td class="num">{r.s.score.kills}</td>
                    <td class="num">{r.s.score.deaths}</td>
                    <td class="num">{r.s.score.assists}</td>
                    <td class="num">{r.s.score.deaths ? (r.s.score.kills / r.s.score.deaths).toFixed(2) : r.s.score.kills.toFixed(2)}</td>
                    <td class="num">{r.spree}</td>
                    <td class="s-medal-cell"><For each={r.medals}>{([id, n]) => <span class="s-medal-mini"><MedalIcon id={id} size={20} />{n > 1 ? <small>{n}</small> : null}</span>}</For></td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Panel>
      <Panel title="Tool of Destruction" right={<Derived />}>
        <table class="s-tod">
          <tbody>
            <For each={weapons()} fallback={<tr><td class="s-dim">No knockouts yet.</td></tr>}>
              {(w) => (
                <tr>
                  <td><span class="s-tod-name">{titleCase(w.weapon)}</span><span class="s-tod-bar"><span style={{ width: `${(w.kills / max()) * 100}%` }} /></span></td>
                  <td class="num">{w.kills} ({(w.share * 100).toFixed(1)}%)</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Panel>
    </div>
  );
}
