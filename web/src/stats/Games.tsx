// Stats home: live games, recent games, a people finder and a server report.

import { For, Show, createMemo, createSignal } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import { live } from "../state/live";
import { endedGames, liveGames, standings, teamStandings, type GameState } from "../state/reducer";
import { Crumbs, Pager, Panel } from "./Layout";
import GameCard from "../components/GameCard";
import { ago, clock, titleCase } from "../lib/format";
import { elapsedSeconds, teamName } from "../lib/game";
import { now } from "../lib/now";

function result(g: GameState): string {
  if (g.team_play) return teamStandings(g).slice(0, 2).map((t) => `${teamName(t.team)} ${t.score}`).join(" – ");
  const s = standings(g)[0];
  return s ? `${s.name} (${s.score.score})` : "–";
}

export default function Games() {
  const navigate = useNavigate();
  const PER = 15;
  const [page, setPage] = createSignal(0);
  const [who, setWho] = createSignal("");
  const games = createMemo(() => liveGames(live.state));
  const ended = createMemo(() => endedGames(live.state, 500));
  const accounts = createMemo(() => Object.values(live.state.accounts));
  const today = createMemo(() => {
    const d = new Date().toDateString();
    const gs = live.state.gameOrder.map((id) => live.state.games[id]!).filter((g) => g && new Date(g.started_at).toDateString() === d);
    return { games: gs.length, kills: gs.reduce((n, g) => n + Object.values(g.scores).reduce((m, s) => m + s.kills, 0), 0) };
  });
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], ["Games"]]} />
      <div class="s-two">
        <aside class="s-left">
          <h2 class="s-h2">h2 live</h2>
          <nav class="s-pills" aria-label="Games">
            <a href="#live">Live Now ({games().length})</a>
            <a href="#recent">Recent Games</a>
            <A href="/stats/players">Players &amp; Leaderboards</A>
            <A href="/stats/events">Event Log</A>
          </nav>
          <Panel title="People Finder">
            <form class="s-finder" onSubmit={(e) => { e.preventDefault(); if (who().trim()) navigate(`/stats/players?q=${encodeURIComponent(who().trim())}`); }}>
              <label for="s-finder">Find by gamertag</label>
              <input id="s-finder" value={who()} onInput={(e) => setWho(e.currentTarget.value)} />
              <button type="submit">search</button>
            </form>
          </Panel>
          <Panel title="Server Report" right={<small>now</small>}>
            <dl class="s-dl s-pad">
              <dt>Accounts</dt><dd>{accounts().length}</dd>
              <dt>Online</dt><dd>{accounts().filter((a) => a.online).length}</dd>
              <dt>Games live</dt><dd>{games().length}</dd>
              <dt>Games today</dt><dd>{today().games}</dd>
              <dt>Knockouts today</dt><dd>{today().kills}</dd>
            </dl>
          </Panel>
        </aside>
        <div class="s-right">
          <h1 class="s-h1">Games</h1>
          <Panel title="Live Now" id="live" right={`${games().length} in progress`}>
            <Show when={games().length} fallback={<p class="s-pad s-dim">No live games on this server right now.</p>}>
              <div class="s-cards"><For each={games()}>{(g) => <GameCard game={g} />}</For></div>
            </Show>
          </Panel>
          <Panel title={<Pager page={page()} pages={Math.max(1, Math.ceil(ended().length / PER))} total={ended().length} per={PER} onPage={setPage} />} id="recent">
            <table class="s-table">
              <thead><tr><th>Game</th><th>Date</th><th>Map</th><th>Playlist</th><th>Result</th><th class="num">Length</th></tr></thead>
              <tbody>
                <For each={ended().slice(page() * PER, page() * PER + PER)} fallback={<tr><td colspan="6" class="s-dim">No finished games yet.</td></tr>}>
                  {(g) => (
                    <tr>
                      <td><A href={`/stats/games/${g.game_id}`}>{titleCase(g.variant ?? g.mode)}</A></td>
                      <td title={g.ended_at ?? ""}>{ago(g.ended_at, now())}</td>
                      <td>{titleCase(g.map)}</td>
                      <td>{g.team_play ? "Team Game" : "Free for All"}</td>
                      <td>{result(g)}</td>
                      <td class="num s-mono">{clock(elapsedSeconds(g, now()))}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Panel>
        </div>
      </div>
    </>
  );
}
