// The stats home ("h2 live Online", /stats): what is happening now (players
// online, live games, open games), the server's numbers for the last day and
// all time, leaderboards over a window, the weapon and map of the week, maps,
// weapons, game types and skulls, an activity heatmap, clan standings and the
// recent games. The windowed numbers come from the server's summary
// (`GET /v1/stats/summary`); the live parts from the live state.

import { For, Show, createMemo, type JSX } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { social } from "../state/social";
import { useSessions } from "../state/sessions";
import { useStatsSummary, type SummaryStatus } from "../state/statsSummary";
import { endedGames, isCampaign, liveGames, type GameState } from "../state/reducer";
import { gameMapTitle } from "../state/maps";
import { isHalo1Map, mapTitle } from "../state/gameart";
import { difficultyName, gameTypeLine } from "../state/gametype";
import { OUTCOME_WORDS, campaignOutcome, countMedals, gameMedals, MEDAL_BY_ID, type MedalId } from "../state/derive";
import {
  DAYS, STATS_WINDOWS, alsoPopular, clanStandings, dailyGames, heatmap, missionOfTheWeek, nonZeroRows, hourName, hoursOf, kd, leaderboards, level, ofTheWeek, peak, shares,
  statsWindow, type Board, type StatsSummary, type StatsWindow, type Totals,
} from "../lib/statsHome";
import { campaignLine, gametype, listed, mapName, pingText, seats, sortSessions, kind as sessionKind } from "../lib/sessions";
import { SKULLS } from "../lib/skulls";
import { titleCase } from "../lib/format";
import { now } from "../lib/now";
import { useMotd } from "../state/motd";
import { motdMessages } from "../lib/motd";
import WeaponIcon from "../components/WeaponIcon";
import SkullIcon from "../components/SkullIcon";
import MedalIcon from "../components/MedalIcon";
import GametypeIcon from "../components/GametypeIcon";
import { Crumbs, Derived, Panel, Strip, Tabs, Title, playerHref } from "./Layout";
import { DidYouKnow, MapPreview, MapThumb, Pills, Who, fireteam, gameTitle } from "./Side";
import { ScoreLine } from "./Games";
import { ClanEmblem } from "./clanBits";
import LiveIdle from "./LiveIdle";
import MapPicture from "../components/MapPicture";
import { clanHref } from "./modes";
import { agoWords, count, num, ratio, usDate } from "./fmt";

const mapTitleOf = (m: { map: string; map_id?: string }) => gameMapTitle({ map: m.map, map_id: m.map_id ?? null });
const skullName = (i: number) => SKULLS.find((s) => s.index === i)?.name ?? `Skull ${i}`;

// ---------- live now ----------

function LiveCard(props: { game: GameState }) {
  const g = () => props.game;
  const href = () => `/stats/games/${g().game_id}`;
  return (
    <A href={href()} class="s-livecard" aria-label={`${gameTitle(g())} on ${gameMapTitle(g())}, live`}>
      <MapThumb game={g()} />
      <span class="s-livecard-text">
        <b><GametypeIcon variant={g().variant} />{gameTitle(g())}</b>
        <span>{gameMapTitle(g())}</span>
        <span class="s-livecard-score">
          <Show when={!isCampaign(g())} fallback={<>{fireteam(g())}</>}><ScoreLine game={g()} /></Show>
        </span>
        <small>{gameTypeLine(g())} · started {agoWords(g().started_at, now())}</small>
      </span>
    </A>
  );
}

function LiveNow(props: { week: StatsSummary | null }) {
  const games = createMemo(() => [...liveGames(live.state)].sort((a, b) =>
    Number(isCampaign(a)) - Number(isCampaign(b)) || Object.keys(b.players).length - Object.keys(a.players).length));
  const online = createMemo(() => Object.values(live.state.accounts).filter((a) => a.online).length);
  const feed = useSessions();
  const open = createMemo(() => sortSessions(feed.items().filter((s) => s.state === "open" && listed(s))));
  return (
    <section id="live" aria-labelledby="s-live-h">
      <div class="s-capbar">
        <span class="s-live-tag">LIVE</span>
        <h2 id="s-live-h" class="s-capbar-h">Live Now</h2>
        <span class="s-capbar-right s-livecounts">
          <span><b>{num(online())}</b> {online() === 1 ? "player" : "players"} online</span>
          <span><b>{num(games().length)}</b> {games().length === 1 ? "game" : "games"} in progress</span>
          <Show when={feed.status() !== "unsupported"}><span><b>{num(open().length)}</b> open {open().length === 1 ? "game" : "games"}</span></Show>
        </span>
      </div>
      <div class="s-matte s-live">
        <For each={games().slice(0, 4)} fallback={<LiveIdle week={props.week} />}>
          {(g) => <LiveCard game={g} />}
        </For>
        <Show when={games().length > 4}><p class="s-items-more"><A href="/stats/games">{games().length - 4} more live {games().length - 4 === 1 ? "game" : "games"}</A></p></Show>
        <Show when={open().length}>
          <div class="s-grid-wrap s-open">
            <table class="s-grid">
              <caption class="sr-only">Open games looking for players</caption>
              <thead><tr><th>Open Games</th><th>Map</th><th class="hide-s">Type</th><th class="num">Players</th></tr></thead>
              <tbody>
                <For each={open().slice(0, 5)}>
                  {(s) => (
                    <tr>
                      <td><Who xuid={s.host_xuid} name={s.host_gamertag} /> <span class="s-dim">{gametype(s)}</span></td>
                      <td><Show when={campaignLine(s, mapTitle)} fallback={<span class="s-who"><Show when={s.map}><MapPreview map={s.map!} mapId={s.map_id} /></Show>{mapName(s, mapTitle)}</span>}>{(c) => <span class="s-who">{c()}</span>}</Show></td>
                      <td class="hide-s">{sessionKind(s)}</td>
                      <td class="num">{seats(s).filled} of {seats(s).max}<Show when={pingText(s)}>{(p) => <span class="s-dim"> · {p()}</span>}</Show></td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        </Show>
      </div>
    </section>
  );
}

// ---------- totals ----------

function Tally(props: { label: string; value: string; sub?: string }) {
  return <div class="s-tally-cell"><dt>{props.label}</dt><dd>{props.value}</dd><Show when={props.sub}><small>{props.sub}</small></Show></div>;
}

function Tallies(props: { title: string; t: Totals; note?: JSX.Element }) {
  return (
    <section class="s-tally" aria-label={props.title}>
      <header><h3>{props.title}</h3><Show when={props.note}><span>{props.note}</span></Show></header>
      <dl>
        <Tally label="Games" value={num(props.t.games)} sub={`${num(props.t.multiplayer)} multiplayer · ${num(props.t.campaign)} campaign`} />
        <Tally label="Kills" value={num(props.t.kills)} />
        <Tally label="Deaths" value={num(props.t.deaths + props.t.campaign_deaths)} sub={props.t.campaign_deaths ? `${num(props.t.campaign_deaths)} in campaign` : undefined} />
        <Tally label="Betrayals" value={num(props.t.betrayals)} />
        <Tally label="Hours Played" value={num(hoursOf(props.t.player_seconds))} />
      </dl>
    </section>
  );
}

// ---------- boards and bars ----------

function BoardBox(props: { board: Board; empty: string }) {
  return (
    <section class="s-sec s-board">
      <header><h3>{props.board.title}</h3><Show when={props.board.note}><small>{props.board.note}</small></Show></header>
      <ol>
        <For each={props.board.rows} fallback={<li class="s-board-empty">{props.empty}</li>}>
          {(r, i) => (
            <li classList={{ "s-board-top": i() === 0 }}>
              <span class="s-board-rank">{i() + 1}</span>
              <Who xuid={r.xuid} name={r.name} />
              <b class="s-board-val">{r.text}</b>
            </li>
          )}
        </For>
      </ol>
    </section>
  );
}

/** Horizontal bars, one series: the bar is the share of the biggest value,
 *  the number sits beside it. */
function Bars<T>(props: { rows: T[]; value: (r: T) => number; label: (r: T) => JSX.Element; name: (r: T) => string; unit: string; empty: string }) {
  const max = () => Math.max(1, ...props.rows.map(props.value));
  return (
    <Show when={props.rows.length} fallback={<p class="s-tod-empty">{props.empty}</p>}>
      <ul class="s-bars">
        <For each={props.rows}>
          {(r) => (
            <li title={`${props.name(r)}: ${num(props.value(r))} ${props.unit}`}>
              <span class="s-bars-label">{props.label(r)}</span>
              <span class="s-bars-track"><span class="s-bars-bar" style={{ width: `${(props.value(r) / max()) * 100}%` }} /></span>
              <span class="s-bars-num">{num(props.value(r))}</span>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

function OfTheWeek(props: { sum: StatsSummary | null }) {
  const pick = createMemo(() => (props.sum ? ofTheWeek(props.sum) : { weapon: null, map: null }));
  const mission = createMemo(() => (props.sum && !pick().weapon ? missionOfTheWeek(props.sum) : null));
  const others = createMemo(() => (props.sum ? alsoPopular(props.sum, pick().map) : []));
  const weaponShare = () => {
    const w = pick().weapon, t = props.sum?.totals.kills ?? 0;
    return w && t ? Math.round((w.kills / t) * 100) : 0;
  };
  const mapShare = (games: number) => {
    const t = props.sum?.totals.games ?? 0;
    return t ? Math.round((games / t) * 100) : 0;
  };
  return (
    <div class="s-week">
      <Show when={pick().weapon} fallback={
        <Show when={mission()}>
          {(m) => (
            <Panel title="Mission of the Week" sub="most played campaign level, last 7 days" class="s-week-box">
              <div class="s-week-body">
                <span class="s-week-art"><MapPreview map={m().map} mapId={m().map_id} /></span>
                <div><b>{mapTitleOf(m())}</b><span>{num(m().games)} {m().games === 1 ? "run" : "runs"}</span><span class="s-dim">{mapShare(m().games)}% of all games</span></div>
              </div>
            </Panel>
          )}
        </Show>
      }>
        {(w) => (
          <Panel title="Weapon of the Week" sub="most kills, last 7 days" class="s-week-box">
            <div class="s-week-body">
              <span class="s-week-art s-week-art--weapon"><WeaponIcon weapon={w().weapon} width={112} /></span>
              <div><b>{titleCase(w().weapon)}</b><span>{num(w().kills)} kills</span><span class="s-dim">{weaponShare()}% of all kills</span></div>
            </div>
          </Panel>
        )}
      </Show>
      <Panel title="Map of the Week" sub="most games played, last 7 days" class="s-week-box">
        <Show when={pick().map} fallback={<p class="s-recent-empty">No games this week yet.</p>}>
          {(m) => (
            <>
              <div class="s-week-body">
                <span class="s-week-art"><MapPreview map={m().map} mapId={m().map_id} /></span>
                <div>
                  <b>{mapTitleOf(m())}</b>
                  <span>{num(m().games)} {m().games === 1 ? "game" : "games"}</span>
                  <span class="s-dim">{m().kills ? `${num(m().kills)} kills` : `${mapShare(m().games)}% of all games`}</span>
                </div>
              </div>
              <Show when={others().length}>
                <div class="s-week-also">
                  <h4>Also popular</h4>
                  <ul>
                    <For each={others()}>
                      {(o) => <li title={`${mapTitleOf(o)}: ${num(o.games)} ${o.games === 1 ? "game" : "games"}`}><MapPicture map={o.map} mapId={o.map_id} class="s-mappic" fallback={<span class="s-week-also-none" />} /><span>{mapTitleOf(o)}</span><small>{num(o.games)}</small></li>}
                    </For>
                  </ul>
                </div>
              </Show>
            </>
          )}
        </Show>
      </Panel>
    </div>
  );
}

/** The newest campaign level finished, Legendary or Heroic first (within
 *  the loaded games), with its art: a young server's best story. */
function LatestClear() {
  const clear = createMemo(() => {
    const done = endedGames(live.state, 200, "campaign").filter((g) => campaignOutcome(g) === "complete");
    return done.find((g) => g.difficulty === "legendary" || g.difficulty === "heroic") ?? done[0] ?? null;
  });
  return (
    <Show when={clear()}>
      {(g) => (
        <Panel title="Latest Campaign Clear" sub={g().difficulty === "legendary" || g().difficulty === "heroic" ? `newest on ${difficultyName(g().difficulty)}` : "newest level finished"} class="s-clear">
          <A href={`/stats/games/${g().game_id}`} class="s-clear-body">
            <MapThumb game={g()} />
            <span>
              <b>{gameMapTitle(g())}</b>
              <Show when={difficultyName(g().difficulty)}>{(d) => <span class={`s-clear-diff s-clear-diff--${g().difficulty}`}>{d()}</span>}</Show>
              <small>{fireteam(g())}</small>
              <small>{agoWords(g().ended_at ?? g().started_at, now())}</small>
            </span>
          </A>
        </Panel>
      )}
    </Show>
  );
}

/** Games started per day, a small column chart under the Daily Count. */
function DailySpark(props: { sum: StatsSummary | null }) {
  const days = createMemo(() => (props.sum ? dailyGames(props.sum.activity, now(), 14) : []));
  const max = () => Math.max(1, ...days().map((d) => d.games));
  const total = () => days().reduce((a, d) => a + d.games, 0);
  return (
    <Show when={total() > 0}>
      <figure class="s-spark">
        <figcaption><span>Games per day</span><small>last 14 days</small></figcaption>
        <ol aria-label="Games started per day, last 14 days">
          <For each={days()}>
            {(d, i) => (
              <li title={`${usDate(d.day.toISOString())}: ${d.games} ${d.games === 1 ? "game" : "games"}`} classList={{ today: i() === days().length - 1 }}>
                <i style={{ height: `${d.games ? Math.max(8, (d.games / max()) * 100) : 0}%` }} />
                <span class="sr-only">{usDate(d.day.toISOString())}: {d.games}</span>
              </li>
            )}
          </For>
        </ol>
      </figure>
    </Show>
  );
}

function Heatmap(props: { sum: StatsSummary }) {
  const hm = createMemo(() => heatmap(props.sum.activity));
  const top = createMemo(() => peak(hm().grid));
  return (
    <section class="s-sec s-heat">
      <header>
        <h3>When People Play</h3>
        <small>games started, by day and hour (your time){props.sum.window === "all" ? ", last 30 days" : ""}</small>
      </header>
      <Show when={hm().total} fallback={<p class="s-tod-empty">No games in this window yet.</p>}>
        <div class="s-heat-wrap">
          <table class="s-heat-grid">
            <caption class="sr-only">Games started per day of the week and hour of the day</caption>
            <thead><tr><td /><For each={Array.from({ length: 24 }, (_, h) => h)}>{(h) => <th scope="col">{h % 6 === 0 ? hourName(h).replace(" ", "") : <span class="sr-only">{hourName(h)}</span>}</th>}</For></tr></thead>
            <tbody>
              <For each={hm().grid}>
                {(row, d) => (
                  <tr>
                    <th scope="row">{DAYS[d()]}</th>
                    <For each={row}>
                      {(v, h) => <td class={`lv${level(v, hm().max)}`} title={`${DAYS[d()]} ${hourName(h())}: ${v} ${v === 1 ? "game" : "games"}`}><span class="sr-only">{v}</span></td>}
                    </For>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
        <div class="s-heat-foot">
          <Show when={top()}>{(p) => <span>Busiest: <b>{DAYS[p().day]} {hourName(p().hour)}</b> ({p().value} {p().value === 1 ? "game" : "games"})</span>}</Show>
          <span class="s-heat-key" aria-hidden="true">fewer <i class="lv1" /><i class="lv2" /><i class="lv3" /><i class="lv4" /> more</span>
        </div>
      </Show>
    </section>
  );
}

function GameTypes(props: { sum: StatsSummary }) {
  const t = () => props.sum.totals;
  const rows = createMemo(() => [
    { name: "Matchmaking", n: t().matchmaking },
    { name: "Custom Games", n: t().custom },
    { name: "Campaign (solo)", n: Math.max(0, t().campaign - t().coop) },
    { name: "Campaign Co-op", n: t().coop },
  ].filter((r) => r.n > 0));
  return (
    <section class="s-sec">
      <header><h3>Game Types</h3><small>{num(t().games)} games</small></header>
      <Bars rows={rows()} value={(r) => r.n} name={(r) => r.name} label={(r) => r.name} unit="games" empty="No games in this window yet." />
      <Show when={props.sum.playlists.length}>
        <h4 class="s-sec-sub">Playlists</h4>
        <Bars rows={props.sum.playlists.slice(0, 5)} value={(r) => r.games} name={(r) => r.playlist} label={(r) => r.playlist} unit="games" empty="" />
      </Show>
    </section>
  );
}

function Skulls(props: { sum: StatsSummary }) {
  const rows = () => [...props.sum.skulls].sort((a, b) => b.games - a.games || a.skull - b.skull);
  return (
    <section class="s-sec">
      <header><h3>Skulls On</h3><small>games played with each skull</small></header>
      <Show when={rows().length} fallback={<p class="s-tod-empty">Nobody played with skulls in this window.</p>}>
        <ul class="skulls skulls--names s-home-skulls">
          <For each={rows().slice(0, 10)}>
            {(s) => <li><SkullIcon index={s.skull} size={16} />{skullName(s.skull)} <small>{num(s.games)}</small></li>}
          </For>
        </ul>
      </Show>
    </section>
  );
}

/** Medals from the loaded multiplayer games in the window: derived here,
 *  since the events don't carry medals. */
function TopMedals(props: { since: number }) {
  const counts = createMemo(() => {
    const total = new Map<MedalId, number>();
    for (const g of endedGames(live.state, 500, "multiplayer")) {
      if (Date.parse(g.started_at) < props.since) continue;
      for (const [id, n] of countMedals(gameMedals(g.kills))) total.set(id, (total.get(id) ?? 0) + n);
    }
    return [...total].sort((a, b) => b[1] - a[1]).slice(0, 6);
  });
  return (
    <section class="s-sec">
      <header><h3><Derived note="Counted in your browser from the multiplayer games this page has loaded; the server does not keep medals.">Top Medals</Derived></h3><small>from loaded games</small></header>
      <Show when={counts().length} fallback={<p class="s-tod-empty">No medals earned in the loaded games.</p>}>
        <ul class="s-home-medals">
          <For each={counts()}>
            {([id, n]) => <li title={MEDAL_BY_ID[id].name}><MedalIcon id={id} size={30} /><span>{MEDAL_BY_ID[id].name}</span><b>{num(n)}</b></li>}
          </For>
        </ul>
      </Show>
    </section>
  );
}

function RecentFeed() {
  const games = createMemo(() => endedGames(live.state, 6));
  return (
    <section class="s-sec s-feed">
      <header><h3>Recent Games</h3><A href="/stats/games?games=recent" class="s-block-right">see all games</A></header>
      <For each={games()} fallback={<p class="s-tod-empty">No games on record yet. The lobby is warming up.</p>}>
        {(g) => (
          <A href={`/stats/games/${g.game_id}`} class="s-feed-item">
            <MapThumb game={g} />
            <span>
              <b>{isCampaign(g) ? gameMapTitle(g) : `${gameTitle(g)} on ${gameMapTitle(g)}`}</b>
              <span class="s-feed-type">{gameTypeLine(g)}</span>
              <small>{isCampaign(g) ? `${OUTCOME_WORDS[campaignOutcome(g)]} · ${fireteam(g)}` : count(Object.keys(g.players).length, "player")} · {agoWords(g.ended_at ?? g.started_at, now())}</small>
            </span>
          </A>
        )}
      </For>
    </section>
  );
}

function Clans(props: { sum: StatsSummary }) {
  const rows = createMemo(() => clanStandings(
    social.teams().map((t) => ({ id: t.id, name: t.properties.name, members: Object.keys(t.members) })),
    props.sum.players,
  ));
  const team = (id: string) => social.teams().find((t) => t.id === id);
  return (
    <Panel title="Clan Standings" sub="members' combined multiplayer kills" class="s-clanbox">
      <Show when={rows().length} fallback={<p class="s-recent-empty">No clan member has played in this window.</p>}>
        <ol class="s-mini-list s-clan-standings">
          <For each={rows()}>
            {(c, i) => (
              <li>
                <span class="s-board-rank">{i() + 1}</span>
                <Show when={team(c.id)}>{(t) => <ClanEmblem team={t()} size={14} />}</Show>
                <A href={clanHref(c.id)}>{c.name}</A>
                <span class="s-dim" title={`${c.active} of ${c.members} members played; K/D ${ratio(c.kills, c.deaths)}`}>{num(c.kills)} kills</span>
              </li>
            )}
          </For>
        </ol>
      </Show>
    </Panel>
  );
}

// ---------- the page ----------

export default function Home() {
  const [search] = useSearchParams<{ window?: string }>();
  const win = () => statsWindow(search.window);
  // The week's summary feeds the "of the week" boxes and the totals; the chosen window the boards.
  const week = useStatsSummary(() => "7d");
  const chosen = useStatsSummary(() => (win() === "7d" ? null : win()));
  // The last 30 days' hourly activity for the Daily Count's per-day chart.
  const month = useStatsSummary(() => (win() === "30d" ? null : "30d"));
  return (
    <HomeView win={win()} week={week.summary()} weekStatus={week.status()} month={win() === "30d" ? chosen.summary() : month.summary()}
      sum={win() === "7d" ? week.summary() : chosen.summary()} status={win() === "7d" ? week.status() : chosen.status()} />
  );
}

/** The page itself, given the summaries: what the route renders, and what
 *  tests render with made-up numbers. */
export function HomeView(props: { win: StatsWindow; week: StatsSummary | null; weekStatus: SummaryStatus; sum: StatsSummary | null; status: SummaryStatus; month?: StatsSummary | null }) {
  const win = () => props.win;
  const sum = () => props.sum;
  const status = () => props.status;
  const week = { summary: () => props.week, status: () => props.weekStatus };
  const boards = createMemo(() => leaderboards(sum()?.players ?? []));
  const label = () => STATS_WINDOWS.find((w) => w.id === win())!.short;
  const me = () => identity.me();
  const motd = useMotd();
  const tips = createMemo(() => motdMessages(motd()));
  const accounts = createMemo(() => Object.keys(live.state.accounts).length);
  const online = createMemo(() => Object.values(live.state.accounts).filter((a) => a.online).length);
  const since = () => (sum()?.from ? Date.parse(sum()!.from!) : win() === "all" ? 0 : now() - 7 * 86_400_000);
  const mapRows = () => (sum()?.maps ?? []).slice(0, 6);
  const weaponRows = () => (sum()?.weapons ?? []).slice(0, 6);
  return (
    <>
      <Crumbs trail={[["Home"]]} />
      <Strip kind="home" />
      <div class="s-cols s-cols--over">
        <aside class="s-left">
          <Title>h2 live Online</Title>
          <Pills label="h2 live Online" items={[
            { label: `Live Now (${liveGames(live.state).length})`, href: "#live" },
            { label: "Games Spotlight", href: "/stats/games" },
            { label: "Players & Leaderboards", href: "/stats/players" },
            { label: "Clans", href: "/stats/clans" },
            me() ? { label: "My Service Record", href: playerHref(me()!.gamertag) } : { label: "Sign Up for h2 live", href: "/dash/signup" },
          ]} />
          <Panel title="Server Report" sub={`as of ${usDate(new Date(now()).toISOString())}`} class="s-report">
            <Show when={week.summary()} fallback={<p class="s-recent-empty">{week.status() === "loading" ? "Counting…" : "This server doesn't publish its totals."}</p>}>
              {(s) => (
                <>
                  <dl class="s-rows">
                    <For each={nonZeroRows([
                      { label: "Games on Record", n: s().all_time.games, always: true },
                      { label: "Kills on Record", n: s().all_time.kills },
                      { label: "Campaign Games", n: s().all_time.campaign },
                      { label: "Co-op Games", n: s().all_time.coop },
                      { label: "Hours Played", n: hoursOf(s().all_time.player_seconds) },
                      { label: "Legendary Levels", n: s().all_time.legendary },
                      { label: "Accounts", n: accounts(), always: true },
                    ], 6)}>{(r) => <div><dt>{r.label}</dt><dd>{r.value}</dd></div>}</For>
                  </dl>
                  <div class="s-report-sub"><h3>Daily Count</h3><p>last 24 hours</p></div>
                  <div class="s-daily">
                    <span class="s-daily-mark" aria-hidden="true" />
                    <dl class="s-rows">
                      <For each={nonZeroRows([
                        { label: "Games", n: s().last_24h.games, always: true },
                        { label: "Campaign", n: s().last_24h.campaign },
                        { label: "Kills", n: s().last_24h.kills },
                        { label: "K/D (server)", n: s().last_24h.kills + s().last_24h.deaths, text: kd(s().last_24h.kills, s().last_24h.deaths).toFixed(2) },
                        { label: "Campaign Deaths", n: s().last_24h.campaign_deaths },
                        { label: "Hours Played", n: hoursOf(s().last_24h.player_seconds) },
                        { label: "Accounts Online", n: online() },
                      ], 4)}>{(r) => <div><dt>{r.label}</dt><dd>{r.value}</dd></div>}</For>
                    </dl>
                  </div>
                  <DailySpark sum={props.month ?? null} />
                </>
              )}
            </Show>
          </Panel>
          <OfTheWeek sum={week.summary()} />
          <LatestClear />
          <Show when={sum()}>{(s) => <Clans sum={s()} />}</Show>
          <DidYouKnow messages={tips()} />
        </aside>
        <div class="s-right">
          <LiveNow week={week.summary()} />
          <Show when={week.summary()}>
            {(s) => (
              <div class="s-tallies">
                <Tallies title="Last 24 Hours" t={s().last_24h} />
                <Tallies title="All Time" t={s().all_time} />
              </div>
            )}
          </Show>
          <Title class="s-title--tabs" help="Counted by the server from every game's events. Kills, K/D, wins and sprees are multiplayer's; campaign has its own boards.">Leaderboards</Title>
          <Tabs label="Window" active={win()} tabs={STATS_WINDOWS.map((w) => ({ id: w.id, label: w.label, href: w.id === "7d" ? "/stats" : `/stats?window=${w.id}` }))} />
          <div class="s-matte s-home">
            <Show when={sum()} fallback={
              <p class="s-empty">{status() === "unsupported"
                ? "This server doesn't publish leaderboards yet (it needs a newer h2 live server). Live games and recent games still show."
                : status() === "error" ? "Couldn't read the leaderboards. Trying again shortly." : "Counting…"}</p>
            }>
              {(s) => (
                <>
                  <p class="s-home-window">{num(s().totals.games)} games {label()}{s().from ? `, since ${usDate(s().from)}` : ""}.</p>
                  <div class="s-boards">
                    <For each={boards()}>{(b) => <BoardBox board={b} empty={`Nobody yet ${label()}.`} />}</For>
                  </div>
                  <div class="s-home-pair">
                    <section class="s-sec">
                      <header><h3>Most Played Maps</h3><small>games</small></header>
                      <Bars rows={mapRows()} value={(m) => m.games} name={(m) => mapTitleOf(m)} unit="games" empty={`No games ${label()}.`}
                        label={(m) => <span class="s-who"><MapPreview map={m.map} mapId={m.map_id} />{mapTitleOf(m)}{isHalo1Map(m.map) ? <small class="s-dim s-h1"> Halo 1</small> : null}{m.campaign ? <small class="s-dim"> campaign</small> : null}</span>} />
                    </section>
                    <section class="s-sec">
                      <header><h3>Most Used Weapons</h3><small>kills</small></header>
                      <Bars rows={weaponRows()} value={(w) => w.kills} name={(w) => titleCase(w.weapon)} unit="kills" empty={`No kills ${label()}.`}
                        label={(w) => <span class="s-who"><WeaponIcon weapon={w.weapon} width={34} />{titleCase(w.weapon)}</span>} />
                      <Show when={weaponRows().length}>
                        <p class="s-sec-foot">{shares(s().weapons, (w) => w.kills, 1).map((x) => `${titleCase(x.row.weapon)}: ${x.pct}% of kills`)[0]}</p>
                      </Show>
                    </section>
                  </div>
                  <Heatmap sum={s()} />
                  <div class="s-home-pair">
                    <GameTypes sum={s()} />
                    <div class="s-home-stack">
                      <Skulls sum={s()} />
                      <TopMedals since={since()} />
                    </div>
                  </div>
                </>
              )}
            </Show>
            <RecentFeed />
          </div>
        </div>
      </div>
    </>
  );
}
