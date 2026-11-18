// Ranked playlists (docs/live/live-matchmaking.md §8): each playlist's
// leaderboard, and one player's level and experience per playlist with the
// games that moved them. The server works ranks out; this only reads them.

import { For, Show, createSignal } from "solid-js";
import { A, useParams } from "@solidjs/router";
import { ApiError } from "../api/http";
import { live } from "../state/live";
import { poll } from "../state/sessions";
import { DEFAULT_PLAYLISTS, changeText, parseBoard, parseHistory, parseRanks, playlistName, progress, winRate, type BoardRow, type RankEntry, type Standing } from "../lib/ranks";
import { Title, playerHref } from "./Layout";
import { usDateTime } from "./fmt";

type Status = "loading" | "ok" | "unsupported" | "error";
const REFRESH_MS = 15000;

function Level(props: { s: Standing }) {
  return (
    <span class="s-who" title={`${props.s.experience} XP, ${Math.round(progress(props.s) * 100)}% to the next level`}>
      <b>{props.s.level}</b>
    </span>
  );
}

function statusText(s: Status, empty: string): string {
  return s === "loading" ? "Reading ranks…" : s === "unsupported" ? "This server doesn't keep ranks." : s === "error" ? "The server didn't answer." : empty;
}

/** `/stats/ranks/:playlist?`: one ranked playlist's leaderboard. */
export default function RanksPage() {
  const params = useParams();
  const playlist = () => params.playlist ?? "1";
  const [rows, setRows] = createSignal<BoardRow[]>([]);
  const [status, setStatus] = createSignal<Status>("loading");
  poll(async (signal) => {
    const want = playlist();
    try {
      const body = await live.api().leaderboard(want, 100, signal);
      if (playlist() !== want) return;
      setRows(parseBoard(body));
      setStatus("ok");
    } catch (e) {
      if (signal.aborted) return;
      setStatus(e instanceof ApiError && e.status === 404 ? "unsupported" : "error");
    }
  }, () => { playlist(); setRows([]); setStatus("loading"); }, REFRESH_MS);
  return (
    <div class="s-full">
      <Title class="s-title--bar" help="Levels 1 to 50 per ranked playlist. Every ranked game moves experience up or down against your opponents' levels; the level follows the experience.">
        Ranked: {playlistName(playlist())}
      </Title>
      <nav class="s-pad">
        <For each={Object.keys(DEFAULT_PLAYLISTS)}>
          {(id, i) => <>{i() ? " · " : ""}<A href={`/stats/ranks/${id}`}>{playlistName(id)}</A></>}
        </For>
      </nav>
      <div class="s-grid-wrap">
        <table class="s-grid">
          <caption class="sr-only">{playlistName(playlist())} leaderboard</caption>
          <thead><tr><th class="num">#</th><th>Player</th><th class="num">Level</th><th class="num">XP</th><th class="num hide-s">Games</th><th class="num hide-s">Wins</th></tr></thead>
          <tbody>
            <For each={rows()} fallback={<tr><td colspan="6" class="s-dim">{statusText(status(), "Nobody has played this playlist ranked yet.")}</td></tr>}>
              {(r, i) => (
                <tr>
                  <td class="num">{i() + 1}</td>
                  <td><A href={playerHref(r.gamertag)}>{r.gamertag}</A></td>
                  <td class="num"><Level s={r} /></td>
                  <td class="num">{r.experience}</td>
                  <td class="num hide-s">{r.games}</td>
                  <td class="num hide-s">{winRate(r)}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** `/stats/players/:tag/ranks`: a player's ranks and the last games of each. */
export function PlayerRanksPage() {
  const params = useParams();
  const [ranks, setRanks] = createSignal<Array<[string, Standing]>>([]);
  const [history, setHistory] = createSignal<Record<string, RankEntry[]>>({});
  const [status, setStatus] = createSignal<Status>("loading");
  poll(async (signal) => {
    const tag = params.tag ?? "";
    try {
      const list = parseRanks(await live.api().ranks(tag, signal));
      const hist: Record<string, RankEntry[]> = {};
      for (const [id] of list) hist[id] = parseHistory(await live.api().rankHistory(tag, id, signal)).history.slice(0, 10);
      if (params.tag !== tag) return;
      setRanks(list);
      setHistory(hist);
      setStatus("ok");
    } catch (e) {
      if (signal.aborted) return;
      setStatus(e instanceof ApiError && e.status === 404 ? "unsupported" : "error");
    }
  }, () => { params.tag; setRanks([]); setHistory({}); setStatus("loading"); }, REFRESH_MS);
  return (
    <div class="s-full">
      <Title class="s-title--bar" back={[playerHref(params.tag ?? ""), "Service Record"]}>{params.tag}: Ranks</Title>
      <Show when={ranks().length} fallback={<p class="s-pad s-dim">{statusText(status(), "No ranked games yet.")}</p>}>
        <For each={ranks()}>
          {([id, s]) => (
            <section class="s-pad">
              <h2><A href={`/stats/ranks/${id}`}>{playlistName(id)}</A>: level {s.level}</h2>
              <p>{s.experience} XP · highest level {s.highest} · {s.games} games · {winRate(s)} won</p>
              <ul>
                <For each={history()[id] ?? []}>
                  {(e) => <li><A href={`/stats/games/${e.gameId}`}>{usDateTime(e.at)}</A>: {changeText(e)}</li>}
                </For>
              </ul>
            </section>
          )}
        </For>
      </Show>
    </div>
  );
}
