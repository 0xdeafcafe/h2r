// The post game carnage report under a game: places, scores, the kill
// breakdown, medals and weapons for every player, derived from its events.

import { For, Show, createMemo, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import type { GameState, Standing } from "../state/reducer";
import { standings, teamStandings } from "../state/reducer";
import { bestSprees, countMedals, gameMedals, ordinal, places, weaponBreakdown, MEDALS, type MedalId } from "../state/derive";
import Emblem from "../components/Emblem";
import { BotTag } from "../components/bits";
import MedalIcon from "../components/MedalIcon";
import WeaponIcon from "../components/WeaponIcon";
import { colourFor, teamName } from "../lib/game";
import { titleCase } from "../lib/format";
import { Derived, Tabs, Title, playerHref } from "./Layout";

interface Row {
  s: Standing;
  place: number;
  spree: number;
  suicides: number;
  betrayals: number;
  medals: Array<[MedalId, number]>;
}

const spread = (n: number) => (n > 0 ? `+${n}` : String(n));

export default function Carnage(props: { game: GameState }) {
  const [tab, setTab] = createSignal("overview");
  const rows = createMemo(() => {
    const g = props.game;
    const pl = places(g);
    const sprees = bestSprees(g.kills);
    const medals = gameMedals(g.kills);
    const by = new Map<number, Row>();
    for (const s of standings(g)) {
      by.set(s.index, {
        s, place: pl.get(s.index) ?? 0, spree: sprees.get(s.index) ?? 0, suicides: 0, betrayals: 0,
        medals: [...countMedals(medals, new Set([s.index])).entries()],
      });
    }
    for (const k of g.kills) {
      if (k.kind === "suicide") { const r = by.get(k.victim); if (r) r.suicides++; }
      if (k.kind === "betrayal" && k.killer !== null) { const r = by.get(k.killer); if (r) r.betrayals++; }
    }
    return by;
  });
  // Teams (or one unnamed group for free-for-all), each with its players in place order.
  const groups = createMemo(() => {
    const g = props.game;
    const order = (a: Row, b: Row) => a.place - b.place || b.s.score.score - a.s.score.score || a.s.index - b.s.index;
    if (!g.team_play) return [{ team: null as number | null, score: 0, place: 0, players: [...rows().values()].sort(order) }];
    return teamStandings(g).map((t) => {
      const players = t.players.map((p) => rows().get(p.index)!).filter(Boolean).sort((a, b) => b.s.score.score - a.s.score.score);
      return { team: t.team as number | null, score: t.score, place: players[0]?.place ?? 0, players };
    });
  });
  const totalMedals = createMemo(() => [...countMedals(gameMedals(props.game.kills)).entries()].sort((a, b) => b[1] - a[1]));
  const weapons = createMemo(() => weaponBreakdown(props.game.kills));
  const max = () => Math.max(0.0001, ...weapons().map((w) => w.share));
  const ended = () => props.game.status === "ended";

  const Who = (p: { r: Row }) => (
    <td>
      <span class="who">
        <Show when={!p.r.s.bot && p.r.s.xuid} fallback={<><Emblem seed={p.r.s.xuid ?? p.r.s.name} appearance={p.r.s.appearance} size={19} /><span>{p.r.s.name}</span></>}>
          {(x) => <><Emblem seed={x()} appearance={p.r.s.appearance} size={19} /><A href={playerHref(p.r.s.name)}>{p.r.s.name}</A></>}
        </Show>
        <Show when={p.r.s.bot}><BotTag /></Show>
        <Show when={p.r.s.game_name}>{(n) => <small class="who-game-name" title="Name in the game">{n()}</small>}</Show>
        <Show when={p.r.s.left}><small>(left)</small></Show>
      </span>
    </td>
  );
  const rowStyle = (r: Row) => ({ "--row": colourFor(props.game, r.s.team, r.s.index) });
  const teamStyle = (team: number) => ({ "--row": colourFor(props.game, team, 0) });
  const sum = (rs: Row[], f: (r: Row) => number) => rs.reduce((n, r) => n + f(r), 0);

  return (
    <section class="s-game-report" aria-label="Carnage report">
      <Title right={<Derived note="Places, sprees, medals and weapon counts are derived in your browser from the game's events. Snapshots carry each game's last 50 knockouts." />}>
        {ended() ? "Post Game Carnage Report" : "Carnage Report So Far"}
      </Title>
      <Tabs small label="Carnage report" active={tab()} onPick={setTab} tabs={[
        { id: "overview", label: "Overview" }, { id: "carnage", label: "Carnage" }, { id: "medals", label: "Medals" }, { id: "weapons", label: "Weapons" },
      ]} />
      <div class="s-carnage" role="tabpanel" aria-label={`${tab()} report`}>
        <p class="s-report-note">{ended() ? "Final standings" : "Live standings"}<span>Medals and weapon counts cover the loaded game events.<Derived /></span></p>
        <Show when={tab() === "overview"}>
          <div class="s-carnage-cols">
            <table class="s-stats s-overview-table" aria-label="Game standings">
              <colgroup><col /><col style={{ width: "70px" }} /><col style={{ width: "70px" }} /></colgroup>
              <thead><tr><th>Players</th><th>Place</th><th>Score</th></tr></thead>
              <tbody>
                <For each={groups()}>
                  {(grp) => (
                    <>
                      <Show when={grp.team !== null}>
                        <tr class="team" style={teamStyle(grp.team!)}><td>{teamName(grp.team!)} Team</td><td>{ended() ? ordinal(grp.place) : grp.place}</td><td>{grp.score}</td></tr>
                      </Show>
                      <For each={grp.players}>
                        {(r) => (
                          <tr classList={{ gone: r.s.left }} style={rowStyle(r)}>
                            <Who r={r} />
                            <td>{grp.team !== null ? "-" : ended() ? ordinal(r.place) : r.place}</td>
                            <td>{r.s.score.score}</td>
                          </tr>
                        )}
                      </For>
                    </>
                  )}
                </For>
              </tbody>
            </table>
            <div class="s-carnage-extras">
              <section class="s-sec">
                <header><span>Total Game Medals</span><Derived /></header>
                <div class="s-game-medals">
                  <div class="s-game-medal-grid">
                    <For each={totalMedals()} fallback={<p class="s-dim" style={{ "grid-column": "1 / -1", padding: "8px 2px" }}>No medals yet.</p>}>
                      {([id, n]) => <div class="s-square"><span><MedalIcon id={id} size={26} /></span><b>{n}</b></div>}
                    </For>
                  </div>
                </div>
              </section>
              <section class="s-sec">
                <header><span>Tool of Destruction</span><Derived /></header>
                <div class="s-game-weapon-key"><span>Weapon / share of kills</span><span>Kills</span></div>
                <div class="s-tod s-game-weapons">
                  <For each={weapons().slice(0, 4)} fallback={<p class="s-tod-empty">No knockouts yet.</p>}>
                    {(w) => (
                      <div class="s-tod-row">
                        <span class="s-tod-ico"><span><WeaponIcon weapon={w.weapon} width={50} /><small>{titleCase(w.weapon)}</small></span></span>
                        <div class="s-tod-bars"><div class="s-tod-bar"><span style={{ width: `${(w.share / max()) * 74}%` }} />{(w.share * 100).toFixed(1)}%</div></div>
                        <div class="s-tod-nums">{w.kills}</div>
                      </div>
                    )}
                  </For>
                </div>
              </section>
            </div>
          </div>
        </Show>
        <Show when={tab() === "carnage"}>
          <div class="s-scroll" tabindex="0" role="region" aria-label="Carnage">
            <table class="s-stats s-carnage-table" aria-label="Player carnage statistics">
              <colgroup><col class="s-player-col" /><col span="7" /></colgroup>
              <thead><tr><th>Players</th><th>Kills</th><th>Assists</th><th>Deaths</th><th>K/D Spread</th><th>Suicides</th><th>Betrayals</th><th>Score</th></tr></thead>
              <tbody>
                <For each={groups()}>
                  {(grp) => (
                    <>
                      <Show when={grp.team !== null}>
                        <tr class="team" style={teamStyle(grp.team!)}>
                          <td>{teamName(grp.team!)} Team</td>
                          <td>{sum(grp.players, (r) => r.s.score.kills)}</td><td>{sum(grp.players, (r) => r.s.score.assists)}</td><td>{sum(grp.players, (r) => r.s.score.deaths)}</td>
                          <td>{spread(sum(grp.players, (r) => r.s.score.kills - r.s.score.deaths))}</td>
                          <td>{sum(grp.players, (r) => r.suicides)}</td><td>{sum(grp.players, (r) => r.betrayals)}</td><td>{grp.score}</td>
                        </tr>
                      </Show>
                      <For each={grp.players}>
                        {(r) => (
                          <tr classList={{ gone: r.s.left }} style={rowStyle(r)}>
                            <Who r={r} />
                            <td>{r.s.score.kills}</td><td>{r.s.score.assists}</td><td>{r.s.score.deaths}</td>
                            <td>{spread(r.s.score.kills - r.s.score.deaths)}</td>
                            <td>{r.suicides}</td><td>{r.betrayals}</td><td>{r.s.score.score}</td>
                          </tr>
                        )}
                      </For>
                    </>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        </Show>
        <Show when={tab() === "medals"}>
          <div class="s-scroll" tabindex="0" role="region" aria-label="Player medals">
            <table class="s-stats s-medals-table" aria-label="Player medals and sprees">
              <colgroup><col class="s-player-col" /><col style={{ width: "80px" }} /><col /></colgroup>
              <thead><tr><th>Players</th><th>Best Spree</th><th style={{ "text-align": "left" }}>Medals</th></tr></thead>
              <tbody>
                <For each={groups()}>
                  {(grp) => (
                    <>
                      <Show when={grp.team !== null}>
                        <tr class="team" style={teamStyle(grp.team!)}><td>{teamName(grp.team!)} Team</td><td>{Math.max(0, ...grp.players.map((r) => r.spree))}</td><td style={{ "text-align": "left" }}>{sum(grp.players, (r) => sum2(r.medals))} medals</td></tr>
                      </Show>
                      <For each={grp.players}>
                        {(r) => (
                          <tr classList={{ gone: r.s.left }} style={rowStyle(r)}>
                            <Who r={r} />
                            <td>{r.spree}</td>
                            <td class="medals"><For each={r.medals}>{([id, n]) => <span><MedalIcon id={id} size={21} />{n > 1 ? <small>{n}</small> : null}</span>}</For></td>
                          </tr>
                        )}
                      </For>
                    </>
                  )}
                </For>
              </tbody>
            </table>
          </div>
          <section class="s-sec s-game-medal-chest">
            <header><span>Total Game Medals</span><Derived /></header>
            <div class="s-game-medal-grid s-game-medal-grid--named">
              <For each={MEDALS}>{(medal) => {
                const count = () => totalMedals().find(([id]) => id === medal.id)?.[1] ?? 0;
                return <div class="s-game-medal" classList={{ empty: !count() }}>
                  <MedalIcon id={medal.id} size={30} dim={!count()} />
                  <span><b>{count()}</b><small>{medal.name}</small></span>
                </div>;
              }}</For>
            </div>
          </section>
        </Show>
        <Show when={tab() === "weapons"}>
          <section class="s-sec">
            <header><span>Tool of Destruction</span><Derived /></header>
            <div class="s-game-weapon-key"><span>Weapon / share of kills</span><span>Kills</span></div>
            <div class="s-tod s-game-weapons">
              <For each={weapons()} fallback={<p class="s-tod-empty">No knockouts yet.</p>}>
                {(w) => (
                  <div class="s-tod-row">
                    <span class="s-tod-ico"><span><WeaponIcon weapon={w.weapon} width={50} /><small>{titleCase(w.weapon)}</small></span></span>
                    <div class="s-tod-bars"><div class="s-tod-bar"><span style={{ width: `${(w.share / max()) * 86}%` }} />{(w.share * 100).toFixed(1)}%</div></div>
                    <div class="s-tod-nums">{w.kills}</div>
                  </div>
                )}
              </For>
            </div>
          </section>
        </Show>
      </div>
    </section>
  );
}

function sum2(m: Array<[MedalId, number]>): number {
  return m.reduce((n, [, x]) => n + x, 0);
}
