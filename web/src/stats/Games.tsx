// Stats home ("h2 live Online"): the featured live map, the games spotlight
// with its tabs, and the menu column with the finder and server reports.

import { gameMapTitle } from "../state/maps";
import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { spotlightTab, type SpotlightTab } from "./modes";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { accountByGamertag, endedGames, isCampaign, liveGames, standings, teamStandings, type GameState } from "../state/reducer";
import { OUTCOME_WORDS, campaignOutcome, deathsOf } from "../state/derive";
import { gameTypeLine } from "../state/gametype";
import MapCanvas from "../components/MapCanvas";
import { colourFor, elapsedSeconds, gameSeconds, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { clock } from "../lib/format";
import { now } from "../lib/now";
import { outlineFor } from "../state/outlines";
import { Crumbs, Pager, Panel, Strip, Tabs, Title, playerHref } from "./Layout";
import GametypeIcon from "../components/GametypeIcon";
import SkullIcon, { SkullBadges } from "../components/SkullIcon";
import { filterBySkulls, formatSkullsParam, parseSkullsParam, skullTitle, skullsOfTier, toggleSkull } from "../lib/skulls";
import { DidYouKnow, MapThumb, PeopleFinder, Pills, RecentGames, fireteam, gameTitle, hostName, playlist } from "./Side";
import { motdMessages } from "../lib/motd";
import { useMotd } from "../state/motd";
import { agoWords, dotDate, num, usDate } from "./fmt";

type Tab = SpotlightTab;

function ScoreLine(props: { game: GameState }) {
  return (
    <span class="s-scoreline">
      <Show
        when={props.game.team_play}
        fallback={<For each={standings(props.game).slice(0, 2)}>{(s) => (
          <span><i class="swatch" style={{ background: colourFor(props.game, s.team, s.index) }} />{s.name} <b>{s.score.score}</b></span>
        )}</For>}
      >
        <For each={teamStandings(props.game).slice(0, 2)}>{(t) => (
          <span><i class="swatch" style={{ background: colourFor(props.game, t.team, 0) }} />{teamName(t.team)} <b>{t.score}</b></span>
        )}</For>
      </Show>
    </span>
  );
}

/** A campaign game in the spotlight: who played, how it went, no scores. */
function CampaignItem(props: { game: GameState }) {
  const g = () => props.game;
  const href = () => `/stats/games/${g().game_id}`;
  const deaths = () => deathsOf(g(), Object.keys(g().players).map(Number));
  return (
    <article class="s-item s-item--campaign" classList={{ "s-item--live": g().status === "live" }}>
      <div class="s-item-main">
        <A href={href()} class="s-item-thumb" aria-label={`${gameTitle(g())} on ${gameMapTitle(g())}`}><MapThumb game={g()} /></A>
        <div class="s-item-text">
          <h4><A href={href()}>{gameMapTitle(g())}</A></h4>
          <p>{gameTypeLine(g())}, {usDate(g().started_at)}</p>
          <p class="s-item-dim">{Object.keys(g().players).length} {Object.keys(g().players).length === 1 ? "player" : "players"}</p>
          <div class="s-item-skulls"><SkullBadges skulls={g().skulls} size={18} /></div>
        </div>
        <div class="s-item-author">
          <div>Players: <b>{fireteam(g())}</b></div>
          <div class="s-item-score">{OUTCOME_WORDS[campaignOutcome(g())]}</div>
          <div class="s-item-dim">{num(deaths())} {deaths() === 1 ? "death" : "deaths"}<br />{g().status === "live" ? `${clock(elapsedSeconds(g(), now()))} played` : `${clock(gameSeconds(g(), now()))} long`}</div>
        </div>
      </div>
      <footer>
        <i>{g().status === "live" ? `started ${agoWords(g().started_at, now())}` : `played ${dotDate(g().ended_at ?? g().started_at)}`}</i>
        <span>
          <Show when={g().status === "live"} fallback={<><A href={href()}>Mission Report</A><b>|</b><A href={href()}>Watch Replay</A></>}>
            <A href={href()}>Watch Live</A>
          </Show>
          <b>|</b><A href={href()}>View Game</A>
        </span>
      </footer>
    </article>
  );
}

function GameItem(props: { game: GameState }) {
  return <Show when={isCampaign(props.game)} fallback={<MultiplayerItem game={props.game} />}><CampaignItem game={props.game} /></Show>;
}

function MultiplayerItem(props: { game: GameState }) {
  const g = () => props.game;
  const href = () => `/stats/games/${g().game_id}`;
  const host = () => hostName(g());
  const kills = () => Object.values(g().scores).reduce((n, s) => n + s.kills, 0);
  return (
    <article class="s-item" classList={{ "s-item--live": g().status === "live" }}>
      <div class="s-item-main">
        <A href={href()} class="s-item-thumb" aria-label={`${gameTitle(g())} on ${gameMapTitle(g())}`}><MapThumb game={g()} /></A>
        <div class="s-item-text">
          <h4><GametypeIcon variant={g().variant} /><A href={href()}>{gameTitle(g())}</A></h4>
          <p>{playlist(g())} on {gameMapTitle(g())}, {usDate(g().started_at)}</p>
          <p class="s-item-dim">{Object.keys(g().players).length} players{g().score_limit ? `, first to ${g().score_limit}` : ""}</p>
          <div class="s-item-skulls"><SkullBadges skulls={g().skulls} size={18} /></div>
        </div>
        <div class="s-item-author">
          <div>{g().status === "live" ? "Host" : "Winner"}: <Show when={g().status === "live"} fallback={<b>{winnerName(g())}</b>}>
            <Show when={host()} fallback="–">{(h) => <Show when={accountByGamertag(live.state, h())} fallback={h()}><A href={playerHref(h())}>{h()}</A></Show>}</Show></Show>
          </div>
          <div class="s-item-score"><ScoreLine game={g()} /></div>
          <div class="s-item-dim">{num(kills())} knockouts<br />{g().status === "live" ? `${clock(elapsedSeconds(g(), now()))} played` : `${clock(gameSeconds(g(), now()))} long`}</div>
        </div>
      </div>
      <footer>
        <i>{g().status === "live" ? `started ${agoWords(g().started_at, now())}` : `played ${dotDate(g().ended_at ?? g().started_at)}`}</i>
        <span>
          <Show when={g().status === "live"} fallback={<><A href={href()}>Carnage Report</A><b>|</b><A href={href()}>Watch Replay</A></>}>
            <A href={href()}>Watch Live</A><b>|</b><A href={href()}>Scoreboard</A>
          </Show>
          <b>|</b><A href={href()}>View Game</A>
        </span>
      </footer>
    </article>
  );
}

function winnerName(g: GameState): string {
  const w = g.winner;
  if (!w) return g.end_reason === "quit" ? "ended early" : "no winner";
  return "team" in w ? `${teamName(w.team)} Team` : g.players[w.player]?.name ?? "–";
}

/** The big picture at the top of the page: the busiest live game, as it happens. */
function Featured(props: { game: GameState | undefined; online: number }) {
  return (
    <>
      <div class="s-capbar">
        <Show when={props.game} fallback={<span class="s-dim">Nothing live right now</span>}>
          {(g) => (
            <>
              <span class="s-live-tag">LIVE</span>
              <A href={`/stats/games/${g().game_id}`}>{gameTitle(g())} on {gameMapTitle(g())}</A>
              <span class="s-capbar-right"><Show when={!isCampaign(g())} fallback={fireteam(g())}><ScoreLine game={g()} /></Show></span>
            </>
          )}
        </Show>
      </div>
      <div class="s-world">
        <Show when={props.game} keyed fallback={<div class="s-world-empty">No live games on this server right now.</div>}>
          {(g) => {
            const roster = createMemo(() => rosterFor(g));
            const outline = outlineFor(() => g.map);
            return (
              <A href={`/stats/games/${g.game_id}`} class="s-world-map" aria-label={`Watch ${gameTitle(g)} on ${gameMapTitle(g)} live`}>
                <MapCanvas frame={liveFrameSource(g.game_id, roster)} roster={roster} outline={outline} label={`Live map of ${gameMapTitle(g)}`} />
              </A>
            );
          }}
        </Show>
        <span class="s-world-count">{props.online} players online</span>
      </div>
    </>
  );
}

/** Toggle chips for the skulls: a game must have had all the chosen ones on. */
function SkullFilter(props: { wanted: number[]; onChange: (next: number[]) => void }) {
  return (
    <div class="s-skull-filter" role="group" aria-label="Filter by skulls">
      <For each={(["gold", "silver"] as const)}>
        {(tier) => (
          <div class="s-skull-tier" role="group" aria-label={`${tier === "gold" ? "Gold" : "Silver"} skulls`}>
            <span>{tier === "gold" ? "Gold" : "Silver"}</span>
            <For each={skullsOfTier(tier)}>
              {(s) => (
                <button type="button" class="s-skull-chip" aria-pressed={props.wanted.includes(s.index)} title={skullTitle(s.index)}
                  onClick={() => props.onChange(toggleSkull(props.wanted, s.index))}>
                  <SkullIcon index={s.index} size={18} title={false} />{s.name}
                </button>
              )}
            </For>
          </div>
        )}
      </For>
      <Show when={props.wanted.length}><button type="button" class="s-linkbtn" onClick={() => props.onChange([])}>clear</button></Show>
    </div>
  );
}

export default function Games() {
  const PER = 8;
  // The tab rides the URL (`?games=campaign`), so a campaign list can be linked.
  const [search, setSearch] = useSearchParams<{ games?: string; skulls?: string }>();
  const [page, setPage] = createSignal(0);
  // The skull filter rides the URL too (`?skulls=10,14`), as it does the API call.
  const wanted = createMemo(() => parseSkullsParam(search.skulls));
  const pickSkulls = (next: number[]) => { setSearch({ skulls: formatSkullsParam(next) }, { replace: true, scroll: false }); setPage(0); };
  // With a filter on, ask the server for the matching games, which reaches past the snapshot's newest 100.
  createEffect(() => {
    const w = wanted();
    if (!w.length) return;
    const ac = new AbortController();
    onCleanup(() => ac.abort());
    for (const state of ["live", "ended"] as const) {
      live.api().games(state, 100, ac.signal, w).then((p) => { if (!ac.signal.aborted) live.ingestGames(p.items); }, () => {});
    }
  });
  const games = createMemo(() => liveGames(live.state));
  // Multiplayer by default; campaign games have their own tab.
  const ended = createMemo(() => endedGames(live.state, 500, "multiplayer"));
  const campaign = createMemo(() => [...liveGames(live.state, "campaign"), ...endedGames(live.state, 500, "campaign")]);
  const accounts = createMemo(() => Object.values(live.state.accounts));
  const online = createMemo(() => accounts().filter((a) => a.online).length);
  const active = (): Tab => spotlightTab(search.games, games().length > 0);
  // The busiest live game, multiplayer before campaign.
  const featured = createMemo(() => [...games()].sort((a, b) =>
    Number(isCampaign(a)) - Number(isCampaign(b)) || Object.keys(b.players).length - Object.keys(a.players).length)[0]);
  // Knockouts and game counts are multiplayer's; campaign games are counted apart.
  const today = createMemo(() => {
    const d = new Date().toDateString();
    const gs = live.state.gameOrder.map((id) => live.state.games[id]!).filter((g) => g && new Date(g.started_at).toDateString() === d);
    const mp = gs.filter((g) => !isCampaign(g));
    return { games: mp.length, campaign: gs.length - mp.length, kills: mp.reduce((n, g) => n + Object.values(g.scores).reduce((m, s) => m + s.kills, 0), 0) };
  });
  const totals = createMemo(() => {
    let kills = 0, n = 0, campaign = 0;
    for (const id of live.state.gameOrder) {
      const g = live.state.games[id];
      if (!g) continue;
      if (isCampaign(g)) { campaign++; continue; }
      n++;
      for (const s of Object.values(g.scores)) kills += s.kills;
    }
    return { kills, games: n, campaign };
  });
  const list = createMemo(() => filterBySkulls(unfiltered(), wanted(), (g) => g.skulls));
  const unfiltered = createMemo(() => {
    switch (active()) {
      // Live games first; recent ones fill the rest of the page.
      case "live": return [...games(), ...ended().slice(0, Math.max(0, PER - games().length))];
      case "team": return ended().filter((g) => g.team_play);
      case "ffa": return ended().filter((g) => !g.team_play);
      case "campaign": return campaign();
      default: return ended();
    }
  });
  const pages = () => Math.max(1, Math.ceil(list().length / PER));
  const pg = () => Math.min(page(), pages() - 1);
  const me = () => identity.me();
  const motd = useMotd();
  const tips = createMemo(() => motdMessages(motd()));
  return (
    <>
      <Crumbs trail={[["Home", "/"], ["h2 live Online"]]} />
      <Strip kind="home" />
      <div class="s-cols s-cols--over">
        <aside class="s-left">
          <Title>h2 live Online</Title>
          <Pills label="h2 live Online" items={[
            { label: `Live Games (${games().length})`, href: "#spotlight" },
            { label: "Players & Leaderboards", href: "/stats/players" },
            me() ? { label: "My Service Record", href: playerHref(me()!.gamertag) } : { label: "Sign Up for h2 live", href: "/dash/signup" },
            { label: "Signed Event Log", href: "/stats/events" },
          ]} />
          <PeopleFinder />
          <Panel title="Server Report" sub={`as of ${usDate(new Date(now()).toISOString())}`} class="s-report">
            <dl class="s-rows">
              <div><dt>Knockouts on Record</dt><dd>{num(totals().kills)}</dd></div>
              <div><dt>Games on Record</dt><dd>{num(totals().games)}</dd></div>
              <div><dt>Campaign Games</dt><dd>{num(totals().campaign)}</dd></div>
              <div><dt>Accounts</dt><dd>{num(accounts().length)}</dd></div>
            </dl>
            <div class="s-report-sub"><h3>Daily Count</h3><p>updated as it happens</p></div>
            <div class="s-daily">
              <span class="s-daily-mark" aria-hidden="true" />
              <dl class="s-rows">
                <div><dt>Active Players</dt><dd>{num(online())}</dd></div>
                <div><dt>Games Today</dt><dd>{num(today().games)}</dd></div>
                <div><dt>Campaign Today</dt><dd>{num(today().campaign)}</dd></div>
                <div><dt>Knockouts Today</dt><dd>{num(today().kills)}</dd></div>
              </dl>
            </div>
          </Panel>
          <RecentGames games={ended().slice(0, 12)} title="Recent h2 live Games" />
          <DidYouKnow messages={tips()} />
        </aside>
        <div class="s-right">
          <Featured game={featured()} online={online()} />
          <Title help="Games on this server, newest first. Live games update as they are played." class="s-title--tabs">Games Spotlight</Title>
          <div id="spotlight">
            <Tabs label="Games" active={active()} onPick={(id) => { setSearch({ games: id }, { replace: true, scroll: false }); setPage(0); }} tabs={[
              { id: "live", label: "Live Now" }, { id: "recent", label: "Recent Games" }, { id: "team", label: "Team Games" }, { id: "ffa", label: "Free for All" },
              { id: "campaign", label: `Campaign (${campaign().length})` },
            ]} />
            <div class="s-matte s-items">
              <SkullFilter wanted={wanted()} onChange={pickSkulls} />
              <div class="s-items-more">
                <Show when={pages() > 1} fallback={<span>{list().length} {list().length === 1 ? "game" : "games"}{wanted().length ? " with those skulls" : ""}</span>}>
                  <button class="s-linkbtn" onClick={() => setPage((pg() + 1) % pages())}>click here for more games</button>
                </Show>
              </div>
              <For each={list().slice(pg() * PER, pg() * PER + PER)} fallback={<p class="s-empty">{wanted().length ? "No games with all of those skulls on record." : "No games on record yet. The lobby is warming up."}</p>}>
                {(g) => <GameItem game={g} />}
              </For>
              <Show when={pages() > 1}><div class="s-items-pager"><Pager page={pg()} pages={pages()} total={list().length} per={PER} onPage={setPage} /></div></Show>
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
