// Ranked playlists (docs/live/live-matchmaking.md §8): the side menu of
// playlists the served hopper file holds (Ranked, Unranked, Clankermatch),
// one playlist's header panel and leaderboard, and a player's own ranks
// with the games that moved them. The server works ranks out; this only
// reads them.

import { For, Show, createMemo, createSignal } from "solid-js";
import { A, useParams } from "@solidjs/router";
import { ApiError } from "../api/http";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { endedGames, type GameState } from "../state/reducer";
import { gameMapTitle } from "../state/maps";
import { poll } from "../state/sessions";
import {
  PLAYLIST_GROUPS, boardStats, changeText, defaultPlaylist, levelTier, parseBoard, parseHistory, parseRanks,
  playlistMenu, playlistName, progress, toNext, winRate, yourRank,
  type BoardRow, type PlaylistInfo, type RankEntry, type Standing,
} from "../lib/ranks";
import { BotTag } from "../components/bits";
import Emblem from "../components/Emblem";
import { Crumbs, Derived, Pager, Panel, Strip, Title, playerHref } from "./Layout";
import { PeopleFinder } from "./Side";
import { HOME_CRUMB } from "./modes";
import { num, usDateTime } from "./fmt";
import { STATS_ROOT, statsPath } from "../routes";

type Status = "loading" | "ok" | "unsupported" | "error";
const REFRESH_MS = 15000;
/** How often the playlist menu and its player counts refresh: they move
 *  slowly, and every count is a board read. */
const MENU_REFRESH_MS = 60000;
/** Boards are asked for whole (the server caps a read at a thousand rows)
 *  so ranks are true and the table pages in the browser. */
const BOARD_LIMIT = 1000;
const PER = 25;

const GROUP_WORD: Record<PlaylistInfo["group"], string> = { ranked: "Ranked", unranked: "Unranked", clanker: "Clankermatch" };

function statusText(s: Status, empty: string): string {
  return s === "loading" ? "Reading ranks…" : s === "unsupported" ? "This server doesn't keep ranks." : s === "error" ? "The server didn't answer." : empty;
}

/** A level as a badge: an original shield shape in a colour grade by level
 *  (lib/ranks.levelTier) — the game's own rank art is never used. */
export function LevelBadge(props: { level: number; size?: number }) {
  return (
    <span class={`s-lvl t${levelTier(props.level)}`} style={props.size ? { "--s-lvl": `${props.size}px` } : undefined}>
      {props.level}
    </span>
  );
}

/** XP with a progress bar to the next level. */
export function Xp(props: { s: Standing }) {
  const title = () => {
    const next = toNext(props.s);
    return next === null
      ? `${num(props.s.experience)} XP — level 50, the top of the ladder`
      : `${num(props.s.experience)} XP, ${num(next)} to level ${props.s.level + 1}`;
  };
  return (
    <span class="s-xp" title={title()}>
      <i>{num(props.s.experience)}</i>
      <span class="s-xp-track" aria-hidden="true"><span class="s-xp-fill" style={{ width: `${Math.round(progress(props.s) * 100)}%` }} /></span>
    </span>
  );
}

/** The side menu: the playlists in their three groups, the current one
 *  lit, each with the players ranked on it. */
export function PlaylistMenu(props: { playlists: PlaylistInfo[]; current: string; counts: Readonly<Record<string, number>> }) {
  return (
    <nav class="s-pills s-rank-menu" aria-label="Playlists">
      <For each={PLAYLIST_GROUPS}>
        {([kind, label]) => (
          <Show when={props.playlists.some((p) => p.group === kind)}>
            <h3 class="s-rank-group">{label}</h3>
            <For each={props.playlists.filter((p) => p.group === kind)}>
              {(p) => (
                <A href={`${STATS_ROOT}/ranks/${p.id}`} classList={{ active: p.id === props.current }}>
                  <span class="s-rank-pill-name">{p.name}</span>
                  <Show when={props.counts[p.id] !== undefined} fallback={<span class="s-rank-count s-rank-count--none">·</span>}>
                    <span class="s-rank-count">{num(props.counts[p.id]!)}</span>
                  </Show>
                </A>
              )}
            </For>
          </Show>
        )}
      </For>
    </nav>
  );
}

/** One leaderboard row: place, player (with emblem, bot mark and "you"),
 *  level badge, XP bar, games, wins, win rate. */
export function Row(props: { r: BoardRow; place: number; you: boolean }) {
  const r = () => props.r;
  return (
    <tr classList={{ sel: props.you }}>
      <td class="num">{props.place}</td>
      <td>
        <span class="s-who">
          <Emblem seed={r().xuid} size={16} />
          <A href={playerHref(r().gamertag)}>{r().gamertag}</A>
          {r().bot === true && <BotTag />}
          <Show when={props.you}><span class="s-rank-youtag">you</span></Show>
        </span>
      </td>
      <td class="num"><LevelBadge level={r().level} /></td>
      <td class="num"><Xp s={r()} /></td>
      <td class="num hide-s">{num(r().games)}</td>
      <td class="num hide-s">{num(r().wins)}</td>
      <td class="num">{winRate(r())}</td>
    </tr>
  );
}

/** The panel above the table: what the playlist is, and what its board
 *  adds up to so far. */
export function PlaylistHead(props: { info: PlaylistInfo | undefined; id: string; rows: BoardRow[]; capped: boolean }) {
  const info = () => props.info;
  const name = () => info()?.name ?? playlistName(props.id);
  const stats = () => boardStats(props.rows);
  return (
    <section class="s-box s-rank-box" aria-label={`${name()} playlist`}>
      <header class="s-box-head s-rank-box-head">
        <div>
          <h3>{name()}</h3>
          <Show when={info()?.description}><p>{info()!.description}</p></Show>
        </div>
        <span class={`s-rank-kind s-rank-kind--${info()?.group ?? "ranked"}`}>{GROUP_WORD[info()?.group ?? "ranked"]}</span>
      </header>
      <div class="s-rank-box-body">
        <Show when={info()?.size || info()?.games.length}>
          <p class="s-rank-rules">
            <Show when={info()?.size}><span>{info()!.size}</span></Show>
            <Show when={info()?.games.length}><span>{info()!.games.join(", ")}</span></Show>
          </p>
        </Show>
        <dl class="s-rank-stats">
          <div><dt>Ranked Players</dt><dd>{props.capped ? `${num(stats().players)}+` : num(stats().players)}</dd></div>
          <div>
            <dt>Player Games <Derived note="Every ranked player's games added up — a team game counts once per player. The API does not serve the playlist's game count itself." /></dt>
            <dd>{num(stats().games)}</dd>
          </div>
          <div><dt>Highest Level</dt><dd><LevelBadge level={stats().highest} size={20} /></dd></div>
        </dl>
      </div>
    </section>
  );
}

/** What the empty board says: how a player gets on it, and where the
 *  playlist's games are. */
export function EmptyBoard(props: { info: PlaylistInfo | undefined; id: string; games: GameState[] }) {
  const name = () => props.info?.name ?? playlistName(props.id);
  return (
    <div class="s-rank-empty">
      <Show
        when={props.info?.group !== "unranked"}
        fallback={
          <>
            <h3>{name()} keeps no ranks.</h3>
            <p>Unranked playlists are for parties of any size, with no level or XP to win or lose. Pick a ranked playlist to climb a board — this page's Ranked group.</p>
          </>
        }
      >
        <h3>Nobody has played {name()} ranked yet.</h3>
        <p>
          Get on the board: in game, Xbox Live › OptiMatch › {name()}, play the match out and finish it.
          The server then moves every entrant's experience against their opponents' levels, and this board fills in the moment it does.
        </p>
      </Show>
      <p class="s-rank-empty-games">
        <Show
          when={props.games.length}
          fallback={<>No games of this playlist on record yet either — <A href={statsPath("/games")}>see this server's games</A>.</>}
        >
          This playlist's latest games —{" "}
          <For each={props.games}>
            {(g, i) => <>{i() ? ", " : ""}<A href={`${STATS_ROOT}/games/${g.game_id}`} title={g.playlist || name()}>{gameMapTitle(g)}</A></>}
          </For>{" "}
          — or <A href={statsPath("/games")}>every game on record</A>
        </Show>
      </p>
    </div>
  );
}

/** `/games/halo2/ranks/:playlist?`: one ranked playlist's leaderboard. */
export default function RanksPage() {
  const params = useParams();
  const me = () => identity.me();
  const [menu, setMenu] = createSignal<PlaylistInfo[] | null>(null);
  const [counts, setCounts] = createSignal<Readonly<Record<string, number>>>({});
  const [rows, setRows] = createSignal<BoardRow[]>([]);
  const [status, setStatus] = createSignal<Status>("loading");
  const [page, setPage] = createSignal(0);

  // The served hopper file's playlists and each board's player count,
  // read slowly: every count is a whole board. Until the file reads (or on
  // a server that published none), the built-in lineup stands in.
  poll(async (signal) => {
    const [doc, marks] = await Promise.all([
      live.api().hopperDoc(signal).catch(() => null),
      live.api().clankerHoppers(signal).catch(() => null),
    ]);
    if (signal.aborted) return;
    const playlists = playlistMenu(doc, marks);
    setMenu(playlists);
    const next: Record<string, number> = { ...counts() };
    await Promise.all(playlists.map(async (p) => {
      try {
        const board = parseBoard(await live.api().leaderboard(p.id, BOARD_LIMIT, signal));
        if (!signal.aborted) next[p.id] = board.length;
      } catch {
        /* this playlist's count keeps its last value */
      }
    }));
    if (signal.aborted) return;
    setCounts(next);
  }, () => { live.server().api; setMenu(null); setCounts({}); }, MENU_REFRESH_MS);

  // Which playlist this page shows: the URL's, else the ranked board with
  // the most players on it.
  const playlists = () => menu() ?? playlistMenu(null, null);
  const playlist = () => params.playlist ?? defaultPlaylist(playlists(), counts());
  const info = () => playlists().find((p) => p.id === playlist());
  const names = () => Object.fromEntries(playlists().map((p) => [p.id, p.name]));

  poll(async (signal) => {
    const want = playlist();
    try {
      const body = await live.api().leaderboard(want, BOARD_LIMIT, signal);
      if (playlist() !== want) return;
      setRows(parseBoard(body));
      setStatus("ok");
    } catch (e) {
      if (signal.aborted) return;
      setStatus(e instanceof ApiError && e.status === 404 ? "unsupported" : "error");
    }
  }, () => { playlist(); setRows([]); setStatus("loading"); setPage(0); }, REFRESH_MS);

  // The signed-in player's own standings, for the side panel.
  const [mine, setMine] = createSignal<Array<[string, Standing]>>([]);
  poll(async (signal) => {
    const tag = me()?.gamertag;
    if (!tag) return;
    try {
      const list = parseRanks(await live.api().ranks(tag, signal));
      if (me()?.gamertag === tag && !signal.aborted) setMine(list);
    } catch {
      /* the panel keeps what it had */
    }
  }, () => { me()?.gamertag; setMine([]); }, REFRESH_MS);

  const stats = () => boardStats(rows());
  const capped = () => rows().length >= BOARD_LIMIT;
  const mineRow = () => rows().find((r) => r.xuid === me()?.xuid) ?? null;
  const rank = () => yourRank(rows(), me()?.xuid);
  const pages = () => Math.max(1, Math.ceil(rows().length / PER));
  const pg = () => Math.min(page(), pages() - 1);
  // This playlist's own latest games, for the empty board's links.
  const playlistGames = createMemo(() => endedGames(live.state, 200, "multiplayer").filter((g) => g.playlist_id === playlist()).slice(0, 5));

  return (
    <>
      <Crumbs trail={[HOME_CRUMB, ["Ranked"]]} />
      <Strip kind="players" />
      <div class="s-cols s-cols--over">
        <aside class="s-left">
          <Title>Ranked</Title>
          <PlaylistMenu playlists={playlists()} current={playlist()} counts={counts()} />
          <Show when={me()} keyed>
            {(m) => (
              <Panel title="Your Ranks" sub="level and XP per playlist" right={<A href={`${playerHref(m.gamertag)}/ranks`}>history</A>}>
                <Show when={mine().length} fallback={<p class="s-recent-empty">No ranked games yet. Play a ranked playlist to earn a level.</p>}>
                  <ul class="s-mini-list s-rank-mine">
                    <For each={mine()}>
                      {([id, s]) => (
                        <li>
                          <LevelBadge level={s.level} size={18} />
                          <A href={`${STATS_ROOT}/ranks/${id}`} title={`${num(s.experience)} XP${s.highest > s.level ? `, highest level ${s.highest}` : ""}`}>
                            {playlistName(id, names())}
                          </A>
                          <span class="s-dim">{num(s.experience)} XP</span>
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </Panel>
            )}
          </Show>
          <PeopleFinder />
        </aside>
        <div class="s-right">
          <PlaylistHead info={info()} id={playlist()} rows={rows()} capped={capped()} />
          <Show when={status() === "ok" && me()} keyed>
            {(m) => (
              <Show
                when={mineRow()}
                keyed
                fallback={
                  <div class="s-rank-you s-rank-you--out">
                    Signed in as <b>{m.gamertag}</b>, and not on this board. <A href={statsPath("/games")}>Play a ranked game</A> to earn your place.
                  </div>
                }
              >
                {(r) => (
                  <div class="s-rank-you">
                    Your rank: <b>#{rank()}</b> of {capped() ? `${num(stats().players)}+` : num(stats().players)}
                    <LevelBadge level={r.level} size={18} />
                    {num(r.experience)} XP
                    <span class="s-rank-you-way">
                      <Show when={toNext(r) !== null} fallback={<>top of the ladder</>}>
                        {Math.round(progress(r) * 100)}% to level {r.level + 1}
                      </Show>
                    </span>
                    <A class="s-rank-you-link" href={`${playerHref(m.gamertag)}/ranks`}>your rank history</A>
                  </div>
                )}
              </Show>
            )}
          </Show>
          <Title class="s-title--tabs" help="Level first, then experience, best first — the server's order. Every ranked game moves experience up or down against your opponents' levels; the level follows the experience.">Leaderboard</Title>
          <Show
            when={rows().length || status() !== "ok"}
            fallback={<div class="s-grid-wrap"><EmptyBoard info={info()} id={playlist()} games={playlistGames()} /></div>}
          >
            <div class="s-grid-wrap">
              <Show when={rows().length}>
                <div class="s-grid-pager"><Pager page={pg()} pages={pages()} total={rows().length} per={PER} onPage={setPage} /></div>
              </Show>
              <table class="s-grid s-grid--roomy">
                <caption class="sr-only">{info()?.name ?? playlistName(playlist())} leaderboard</caption>
                <thead>
                  <tr>
                    <th class="num">#</th><th>Player</th><th class="num">Level</th><th class="num">XP</th>
                    <th class="num hide-s">Games</th><th class="num hide-s">Wins</th><th class="num">Win %</th>
                  </tr>
                </thead>
                <tbody>
                  <For each={rows().slice(pg() * PER, pg() * PER + PER)} fallback={
                    <tr><td colspan="7" class="s-dim">{statusText(status(), "")}</td></tr>
                  }>
                    {(r, i) => <Row r={r} place={pg() * PER + i() + 1} you={r.xuid === me()?.xuid} />}
                  </For>
                </tbody>
              </table>
            </div>
          </Show>
        </div>
      </div>
    </>
  );
}

/** `/games/halo2/players/:tag/ranks`: a player's ranks and the last games of each. */
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
              <h2><A href={`${STATS_ROOT}/ranks/${id}`}>{playlistName(id)}</A>: level {s.level}</h2>
              <p>{s.experience} XP · highest level {s.highest} · {s.games} games · {winRate(s)} won</p>
              <ul>
                <For each={history()[id] ?? []}>
                  {(e) => <li><A href={`${STATS_ROOT}/games/${e.gameId}`}>{usDateTime(e.at)}</A>: {changeText(e)}</li>}
                </For>
              </ul>
            </section>
          )}
        </For>
      </Show>
    </div>
  );
}
