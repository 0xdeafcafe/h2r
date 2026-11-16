// Player pages: service record, game history and career stats (with
// compare), all derived from the events the client has seen.

import { gameMapGroupLabel, gameMapTitle } from "../state/maps";
import { mapGroupKey } from "../lib/maps";
import { For, Show, createEffect, createMemo, createSignal, on, untrack, type JSX } from "solid-js";
import { A, useParams, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { accountByGamertag, type AccountState, type GameState } from "../state/reducer";
import {
  MEDALS, OUTCOME_WORDS, RANKS, campaignHistory, countMedals, gameHistory, gameMedals, nextRankAt, ordinal, playedWith, rankIndex,
  serviceRecord, serviceTag, type CampaignRow, type HistoryRow, type MedalId, type ServiceRecord,
} from "../state/derive";
import type { GameMode } from "../state/reducer";
import { difficultyName, gameTypeLine } from "../state/gametype";
import { historyTab, recordHref } from "./modes";
import Emblem from "../components/Emblem";
import MedalIcon from "../components/MedalIcon";
import RankIcon from "../components/RankIcon";
import WeaponIcon from "../components/WeaponIcon";
import { PresenceBadge, VerifySummary } from "../components/bits";
import { Crumbs, Derived, Pager, Panel, Tabs, Title, playerHref } from "./Layout";
import GametypeIcon from "../components/GametypeIcon";
import { SkullBadges } from "../components/SkullIcon";
import { MapPreview, MapThumb, PeopleFinder, Pills, RecentGames, gameTitle, playlist } from "./Side";
import { titleCase } from "../lib/format";
import { gameSeconds, teamName } from "../lib/game";
import { now } from "../lib/now";
import Wordmark from "./Wordmark";
import PlayerSocial from "./PlayerSocial";
import { hms, longDate, num, ratio, usDate, usDateTime } from "./fmt";

function useAccount() {
  const params = useParams<{ tag: string }>();
  const tag = () => decodeURIComponent(params.tag);
  const acct = () => accountByGamertag(live.state, tag());
  // Games older than the snapshot: load the ones the server lists for them.
  createEffect(on(() => [tag(), live.loadMode()] as const, ([t, mode]) => {
    if (mode === "snapshot") return;
    live.api().account(t).then((a) => { for (const g of (a.recent_games ?? []).slice(0, 25)) void live.ensureGame(g.game_id); }, () => {});
  }));
  return { tag, acct };
}

function NotFound(props: { tag: string }) {
  return (
    <Show when={live.loadMode() !== "snapshot"} fallback={<div class="s-loading">Loading service record…</div>}>
      <div class="s-full">
        <Title class="s-title--bar">Player Not Found</Title>
        <p class="s-pad">No one called “{props.tag}” has an account on this server. <A href="/stats/players">Browse players</A></p>
      </div>
    </Show>
  );
}

const topMedals = (m: Map<MedalId, number>, n: number) => [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, n);

/** The identity strip across the top of every player page. */
export function PlayerStrip(props: { account: AccountState; rec: ServiceRecord }) {
  const pins = () => {
    const top = topMedals(props.rec.medals, 8);
    return Array.from({ length: 8 }, (_, i) => top[i] ?? null);
  };
  const next = () => nextRankAt(props.rec.exp);
  return (
    <div class="s-id">
      <A href={playerHref(props.account.gamertag)} class="s-id-emblem"><Emblem seed={props.account.xuid} size={68} /></A>
      <span class="s-id-rank"><RankIcon rank={rankIndex(props.rec.exp)} exp={props.rec.exp} size={40} /></span>
      <div class="s-id-name">
        <h2><A href={playerHref(props.account.gamertag)}>{props.account.gamertag}</A></h2>
        <div>Service Tag: {serviceTag(props.account.gamertag, props.account.xuid)}</div>
        <div><Derived note="Rank comes from EXP: one point per completed game, two more per win. Derived in your browser.">{props.rec.rank}, Grade {rankIndex(props.rec.exp) + 1}</Derived> · <PresenceBadge account={props.account} /></div>
      </div>
      <div class="s-id-nums">
        <dl>
          <dt>Best Spree:</dt><dd>{props.rec.bestSpree}</dd>
          <dt>Total EXP:</dt><dd>{num(props.rec.exp)}</dd>
          <dt>Next Rank:</dt><dd>{next() === null ? "top of the ladder" : <A href={`${playerHref(props.account.gamertag)}/career`}>{next()} EXP</A>}</dd>
        </dl>
      </div>
      <div class="s-id-pins" aria-label="Top medals">
        <For each={pins()}>
          {(p) => <span classList={{ empty: !p }}>{p ? <MedalIcon id={p[0]} size={25} /> : null}</span>}
        </For>
      </div>
      <div class="s-id-mark" aria-hidden="true"><Wordmark height={22} /></div>
    </div>
  );
}

type Which = "record" | "history" | "career" | "campaign";
const TITLES: Record<Which, string> = { record: "Service Record", history: "Game History", career: "Career Stats", campaign: "Campaign Record" };

/** The service record's two halves, as Bungie.net had them: multiplayer
 *  (the default) and campaign. */
export function ModeTabs(props: { gamertag: string; mode: GameMode }) {
  return (
    <Tabs label="Service record" active={props.mode} tabs={[
      { id: "multiplayer", label: "Multiplayer", href: recordHref(props.gamertag, "multiplayer") },
      { id: "campaign", label: "Campaign", href: recordHref(props.gamertag, "campaign") },
    ]} />
  );
}

export function PlayerFrame(props: { which: Which; children: (a: () => AccountState, rec: () => ServiceRecord, hist: () => HistoryRow[]) => JSX.Element }) {
  const { tag, acct } = useAccount();
  const rec = createMemo(() => (acct() ? serviceRecord(live.state, acct()!.xuid) : null));
  const hist = createMemo(() => (acct() ? gameHistory(live.state, acct()!.xuid) : []));
  return (
    <Show when={acct()} fallback={<><Crumbs trail={[["Home", "/stats"], ["Players", "/stats/players"], [tag()]]} /><NotFound tag={tag()} /></>}>
      {(a) => (
        <>
          <Crumbs trail={[["Home", "/stats"], [`h2 live ${TITLES[props.which]}`]]} />
          <PlayerStrip account={a()} rec={rec()!} />
          {untrack(() => props.children(a, () => rec()!, hist))}
        </>
      )}
    </Show>
  );
}

export function PlayedWith(props: { account: AccountState; mode?: GameMode }) {
  const list = createMemo(() => playedWith(live.state, props.account.xuid, props.mode ?? "multiplayer").slice(0, 6));
  return (
    <Panel title={props.mode === "campaign" ? "Co-op Partners" : "Played With"} sub={props.mode === "campaign" ? "people from shared campaign games" : "people from shared games"}>
      <Show when={list().length} fallback={<p class="s-recent-empty">No shared games yet.</p>}>
        <ul class="s-mini-list">
          <For each={list()}>
            {(f) => {
              const a = () => live.state.accounts[f.xuid];
              return (
                <li>
                  <Show when={a()}>{(x) => <><Emblem seed={x().xuid} size={14} /><A href={playerHref(x().gamertag)}>{x().gamertag}</A></>}</Show>
                  <span class="s-dim">{f.games} game{f.games === 1 ? "" : "s"}</span>
                  <Show when={a()?.online}><span class="s-online-dot" title="online" /></Show>
                </li>
              );
            }}
          </For>
        </ul>
      </Show>
    </Panel>
  );
}

export function ServiceRecordPage() {
  return (
    <PlayerFrame which="record">
      {(a, rec, hist) => {
        const base = () => playerHref(a().gamertag);
        const latest = () => hist()[0];
        const medalTotal = () => [...rec().medals.values()].reduce((n, x) => n + x, 0);
        const me = () => identity.me();
        return (
          <div class="s-cols">
            <aside class="s-left">
              <Title>h2 live Service Record</Title>
              <Pills label="Service record" items={[
                { label: `Game History (${hist().length})`, href: `${base()}/history` },
                { label: "Career Stats", href: `${base()}/career` },
                { label: "Campaign Record", href: `${base()}/campaign` },
                ...(me() && me()!.xuid !== a().xuid ? [{ label: "Compare With Me", href: `${base()}/career?compare=${encodeURIComponent(me()!.gamertag)}` }] : []),
                { label: "Players & Leaderboards", href: "/stats/players" },
              ]} />
              <RecentGames games={hist().slice(0, 12).map((h) => h.game)} title="Recent h2 live Games" empty="Their first chapter is still ahead." />
              <PlayedWith account={a()} />
              <PlayerSocial account={a()} />
              <PeopleFinder />
            </aside>
            <div class="s-right">
              <ModeTabs gamertag={a().gamertag} mode="multiplayer" />
              <div class="s-capbar s-capbar--slim">
                <Show when={latest()} fallback={<span class="s-dim">Latest game</span>}>
                  {(h) => (
                    <>
                      <A href={`/stats/games/${h().game.game_id}`}>{gameTitle(h().game)} on {gameMapTitle(h().game)}</A>
                      <span class="s-capbar-right">{h().game.status === "live" ? "playing now" : usDate(h().game.started_at)}</span>
                    </>
                  )}
                </Show>
              </div>
              <div class="s-shot">
                <Show when={latest()} keyed fallback={<div class="s-shot-empty">No games on record yet.</div>}>
                  {(h) => <MapThumb game={h.game} large />}
                </Show>
              </div>
              <Title class="s-title--rule">Player Snapshot</Title>
              <div class="s-snap">
                <div class="s-snap-pic"><Emblem seed={a().xuid} size={112} /></div>
                <dl class="s-facts">
                  <div><dt>Total Games</dt><dd>{num(rec().games)}</dd></div>
                  <div><dt>Completed Games</dt><dd>{num(rec().completed)}</dd></div>
                  <div><dt>Wins</dt><dd>{num(rec().wins)}</dd></div>
                  <div><dt>Best Spree</dt><dd>{rec().bestSpree}</dd></div>
                  <div><dt>Rank</dt><dd><Derived>{rec().rank}</Derived></dd></div>
                  <div><dt>Total EXP</dt><dd><Derived note="One point per completed game, two more per win; derived in your browser.">{num(rec().exp)}</Derived></dd></div>
                </dl>
                <dl class="s-facts">
                  <h4>Combat Report</h4>
                  <div><dt>Knockouts</dt><dd>{num(rec().kills)}</dd></div>
                  <div><dt>Deaths</dt><dd>{num(rec().deaths)}</dd></div>
                  <div><dt>Assists</dt><dd>{num(rec().assists)}</dd></div>
                  <div><dt>K/D Ratio</dt><dd>{ratio(rec().kills, rec().deaths)}</dd></div>
                </dl>
                <div class="s-snap-foot">
                  <span>Player Since {usDate(a().created_at)}</span>
                  <span>Last Played {rec().lastPlayed ? usDate(rec().lastPlayed) : "never"}</span>
                </div>
              </div>
              <section class="s-achieve">
                <header>Recent Medals</header>
                <div class="s-achieve-body">
                  <dl class="s-facts">
                    <div><dt>Earned</dt><dd>{rec().medals.size} of {MEDALS.length}</dd></div>
                    <div><dt>Medal Count</dt><dd><Derived>{num(medalTotal())}</Derived></dd></div>
                    <div><dt>[ <A href={`${base()}/career`}>see the medal chest</A> ]</dt><dd /></div>
                  </dl>
                  <div class="s-achieve-medals">
                    <For each={topMedals(rec().medals, 4)} fallback={<span class="s-dim">None yet.</span>}>
                      {([id, n]) => <span><MedalIcon id={id} size={44} /><small>{n}</small></span>}
                    </For>
                  </div>
                </div>
              </section>
            </div>
          </div>
        );
      }}
    </PlayerFrame>
  );
}

function HistoryTable(props: { rows: HistoryRow[]; selected?: string; onSelect: (id: string) => void }) {
  return (
    <table class="s-grid">
      <thead><tr><th /><th class="num">Date</th><th>Map</th><th class="hide-s">Playlist</th><th class="num">Place</th><th /></tr></thead>
      <tbody>
        <For each={props.rows} fallback={<tr><td colspan="6" class="s-dim">Your first chapter is still ahead.</td></tr>}>
          {(r) => (
            <tr classList={{ sel: props.selected === r.game.game_id }} onClick={() => props.onSelect(r.game.game_id)}>
              <td><span class="s-who"><GametypeIcon variant={r.game.variant} /><A href={`/stats/games/${r.game.game_id}`}>{gameTitle(r.game)}</A><SkullBadges skulls={r.game.skulls} size={16} /></span></td>
              <td class="num">{usDateTime(r.game.started_at)}</td>
              <td><span class="s-who"><MapPreview map={r.game.map} mapId={r.game.map_id} />{gameMapTitle(r.game)}</span></td>
              <td class="hide-s">{playlist(r.game)}</td>
              <td class="num">{r.game.status === "live" ? <span class="s-live-tag">LIVE</span> : r.place ? ordinal(r.place) : "–"}</td>
              <td class="num"><button class="s-linkbtn" onClick={(e) => { e.stopPropagation(); props.onSelect(r.game.game_id); }}>preview</button></td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}

function CampaignTable(props: { rows: CampaignRow[]; selected?: string; onSelect: (id: string) => void }) {
  return (
    <table class="s-grid">
      <thead><tr><th>Level</th><th class="num">Date</th><th class="hide-s">Type</th><th>Result</th><th class="num">Deaths</th><th /></tr></thead>
      <tbody>
        <For each={props.rows} fallback={<tr><td colspan="6" class="s-dim">No campaign games on record yet.</td></tr>}>
          {(r) => (
            <tr classList={{ sel: props.selected === r.game.game_id }} onClick={() => props.onSelect(r.game.game_id)}>
              <td><span class="s-who"><MapPreview map={r.game.map} mapId={r.game.map_id} /><A href={`/stats/games/${r.game.game_id}`}>{gameMapTitle(r.game)}</A><SkullBadges skulls={r.game.skulls} size={16} /></span></td>
              <td class="num">{usDateTime(r.game.started_at)}</td>
              <td class="hide-s">{gameTypeLine(r.game)}</td>
              <td>{r.outcome === "playing" ? <span class="s-live-tag">LIVE</span> : OUTCOME_WORDS[r.outcome]}</td>
              <td class="num">{r.deaths}</td>
              <td class="num"><button class="s-linkbtn" onClick={(e) => { e.stopPropagation(); props.onSelect(r.game.game_id); }}>preview</button></td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}

function CampaignPreview(props: { row: CampaignRow; gamertag: string }) {
  const g = () => props.row.game;
  return (
    <aside class="s-preview">
      <A href={`/stats/games/${g().game_id}`} class="s-preview-pic" style={{ display: "block" }}><MapThumb game={g()} /></A>
      <div class="s-preview-title"><A href={`/stats/games/${g().game_id}`}>{gameMapTitle(g())}</A></div>
      <p>
        {gameTypeLine(g())}<br />
        {longDate(g().started_at)}<br />
        Length: {hms(props.row.seconds)}
      </p>
      <p>
        <b>{props.gamertag}</b> {props.row.outcome === "playing" ? "is playing now" : props.row.outcome === "complete" ? "completed the level" : "didn't finish"}
        {props.row.partners.length ? ` with ${props.row.partners.join(", ")}` : ", solo"}
      </p>
      <dl class="s-facts">
        <div><dt>Result</dt><dd>{OUTCOME_WORDS[props.row.outcome]}</dd></div>
        <div><dt>Difficulty</dt><dd>{difficultyName(g().difficulty) ?? "–"}</dd></div>
        <div><dt>Deaths</dt><dd>{props.row.deaths}</dd></div>
        <Show when={props.row.kills !== null}><div><dt>AI Kills</dt><dd>{props.row.kills}</dd></div></Show>
      </dl>
      <div class="s-preview-head">Game Record ({g().positions.length})</div>
      <ul class="s-pad">
        <li><VerifySummary positions={() => g().positions} /></li>
        <li><A href={`/stats/games/${g().game_id}`}>{g().status === "live" ? "Watch it live" : "Mission report and replay"}</A></li>
      </ul>
    </aside>
  );
}

export function HistoryPage() {
  const PER = 25;
  const [page, setPage] = createSignal(0);
  const [sel, setSel] = createSignal<string | null>(null);
  // The tab rides the URL (`?tab=campaign`); multiplayer by default.
  const [search, setSearch] = useSearchParams<{ tab?: string }>();
  const tab = () => historyTab(search.tab);
  return (
    <PlayerFrame which="history">
      {(a, _rec, hist) => {
        const campaign = createMemo(() => campaignHistory(live.state, a().xuid, now()));
        const list = () => hist().filter((r) => tab() === "all" || (tab() === "team") === r.game.team_play);
        const total = () => (tab() === "campaign" ? campaign().length : list().length);
        const pages = () => Math.max(1, Math.ceil(total() / PER));
        const pg = () => Math.min(page(), pages() - 1);
        const rows = () => list().slice(pg() * PER, pg() * PER + PER);
        const crows = () => campaign().slice(pg() * PER, pg() * PER + PER);
        const chosen = () => rows().find((r) => r.game.game_id === sel()) ?? rows()[0];
        const cchosen = () => crows().find((r) => r.game.game_id === sel()) ?? crows()[0];
        // Rows on show that only a summary knows (older than the account's
        // recent games) are read in full, so their results and numbers fill in.
        createEffect(() => {
          if (live.loadMode() === "snapshot") return;
          for (const r of tab() === "campaign" ? crows() : rows()) if (r.game.status === "ended" && r.game.end_reason === null) void live.ensureGame(r.game.game_id);
        });
        const pager = (foot?: boolean) => (
          <div class="s-grid-pager" classList={{ "s-grid-pager--foot": !!foot }}><Pager page={pg()} pages={pages()} total={total()} per={PER} onPage={setPage} /></div>
        );
        return (
          <div class="s-full">
            <Title class="s-title--bar" back={["Return to Service Record Main", playerHref(a().gamertag)]}>Game History</Title>
            <Tabs label="Game history" active={tab()} onPick={(id) => { setSearch({ tab: id === "all" ? undefined : id }, { replace: true, scroll: false }); setPage(0); }} tabs={[
              { id: "all", label: "Multiplayer" }, { id: "team", label: "Team Games" }, { id: "ffa", label: "Free for All" },
              { id: "campaign", label: `Campaign (${campaign().length})` },
            ]} />
            <Show
              when={tab() === "campaign"}
              fallback={
                <div class="s-history">
                  <div class="s-grid-wrap">
                    {pager()}
                    <HistoryTable rows={rows()} selected={chosen()?.game.game_id} onSelect={setSel} />
                    {pager(true)}
                  </div>
                  <Show when={chosen()} keyed>{(r) => <Preview row={r} gamertag={a().gamertag} />}</Show>
                </div>
              }
            >
              <div class="s-history">
                <div class="s-grid-wrap">
                  {pager()}
                  <CampaignTable rows={crows()} selected={cchosen()?.game.game_id} onSelect={setSel} />
                  {pager(true)}
                </div>
                <Show when={cchosen()} keyed>{(r) => <CampaignPreview row={r} gamertag={a().gamertag} />}</Show>
              </div>
            </Show>
          </div>
        );
      }}
    </PlayerFrame>
  );
}

function Preview(props: { row: HistoryRow; gamertag: string }) {
  const g = () => props.row.game;
  const medals = createMemo(() => [...countMedals(gameMedals(g().kills), new Set(props.row.slots)).entries()].sort((a, b) => b[1] - a[1]));
  const victim = createMemo(() => {
    const m = new Map<number, number>();
    for (const k of g().kills) if (k.killer !== null && props.row.slots.includes(k.killer) && k.kind === "kill") m.set(k.victim, (m.get(k.victim) ?? 0) + 1);
    const top = [...m.entries()].sort((a, b) => b[1] - a[1])[0];
    return top ? g().players[top[0]]?.name : null;
  });
  const length = () => gameSeconds(g(), now());
  return (
    <aside class="s-preview">
      <A href={`/stats/games/${g().game_id}`} class="s-preview-pic" style={{ display: "block" }}><MapThumb game={g()} /></A>
      <div class="s-preview-title"><A href={`/stats/games/${g().game_id}`}>{gameTitle(g())}</A> on {gameMapTitle(g())}</div>
      <p>
        Playlist: {playlist(g())}<br />
        {longDate(g().started_at)}<br />
        Length: {hms(length())}
      </p>
      <p>
        <b>{props.gamertag}</b> {g().status === "live" ? "is playing now" : props.row.place ? `placed ${ordinal(props.row.place)}` : "played"}
        {props.row.team !== null ? ` on ${teamName(props.row.team)} Team` : ""}
      </p>
      <dl class="s-facts">
        <div><dt>Score</dt><dd>{props.row.score}</dd></div>
        <div><dt>Avg. Life</dt><dd><Derived note="Game length divided by lives; derived in your browser.">{hms(length() / (props.row.deaths + 1))}</Derived></dd></div>
        <div><dt>Best Spree</dt><dd>{props.row.bestSpree}</dd></div>
        <div><dt>Total Kills</dt><dd>{props.row.kills}</dd></div>
        <div><dt>Most Killed</dt><dd>{victim() ?? "–"}</dd></div>
      </dl>
      <div class="s-preview-head">Top Medals of {medals().reduce((n, [, x]) => n + x, 0)} Earned <Derived>{""}</Derived></div>
      <div class="s-preview-medals">
        <For each={medals().slice(0, 4)} fallback={<span class="s-dim">None this game.</span>}>{([id, n]) => <span><MedalIcon id={id} size={38} />{n > 1 ? <small>x{n}</small> : null}</span>}</For>
      </div>
      <div class="s-preview-head">Game Record ({g().positions.length})</div>
      <ul class="s-pad">
        <li><VerifySummary positions={() => g().positions} /></li>
        <li><A href={`/stats/games/${g().game_id}`}>{g().status === "live" ? "Watch it live" : "Carnage report and replay"}</A></li>
      </ul>
    </aside>
  );
}

const CHEST: MedalId[][] = [
  ["opening-act", "last-word", "payback"],
  ["twofer", "hat-trick", "full-house", "encore"],
  ["hot-streak", "wildfire", "untouchable"],
];
const MEDAL = Object.fromEntries(MEDALS.map((m) => [m.id, m]));
const perGame = (n: number, games: number) => (games ? n / games : 0).toFixed(3);

export function MedalChest(props: { rec: ServiceRecord; versus?: ServiceRecord | null }) {
  const better = (id: MedalId) => {
    const v = props.versus;
    if (!v) return false;
    const mine = (props.rec.medals.get(id) ?? 0) / Math.max(1, props.rec.games);
    return mine > (v.medals.get(id) ?? 0) / Math.max(1, v.games);
  };
  return (
    <div class="s-chest">
      <div class="s-chest-cap">Medal Count (count per game){props.versus ? " - Player with better per game ratio highlighted" : ""}</div>
      <For each={CHEST}>
        {(group) => (
          <div class="s-chest-group">
            <For each={group}>
              {(id) => {
                const n = () => props.rec.medals.get(id) ?? 0;
                return (
                  <div class="s-medal" classList={{ empty: !n(), best: better(id) }} title={`${MEDAL[id]!.name}: ${MEDAL[id]!.blurb} (derived)`}>
                    <MedalIcon id={id} size={30} dim={!n()} />
                    <div><b>{num(n())} ({perGame(n(), props.rec.games)})</b><small>{MEDAL[id]!.name}</small></div>
                  </div>
                );
              }}
            </For>
          </div>
        )}
      </For>
    </div>
  );
}

type Sort = "with" | "by";

export function ToolOfDestruction(props: { rec: ServiceRecord; sort: Sort; versus?: ServiceRecord | null }) {
  const [all, setAll] = createSignal(false);
  const rows = createMemo(() => {
    const m = new Map<string, { weapon: string; kills: number; ks: number; deaths: number; ds: number }>();
    for (const w of props.rec.weapons) m.set(w.weapon, { weapon: w.weapon, kills: w.kills, ks: w.share, deaths: 0, ds: 0 });
    for (const w of props.rec.weaponsAgainst) {
      const r = m.get(w.weapon) ?? { weapon: w.weapon, kills: 0, ks: 0, deaths: 0, ds: 0 };
      r.deaths = w.kills; r.ds = w.share;
      m.set(w.weapon, r);
    }
    const key = props.sort === "with" ? (r: { kills: number; deaths: number }) => r.kills * 1000 + r.deaths : (r: { kills: number; deaths: number }) => r.deaths * 1000 + r.kills;
    return [...m.values()].sort((a, b) => key(b) - key(a) || a.weapon.localeCompare(b.weapon));
  });
  const max = () => Math.max(0.0001, ...rows().map((r) => Math.max(r.ks, r.ds)));
  const better = (weapon: string, kills: number) => {
    const v = props.versus;
    if (!v) return false;
    const theirs = v.weapons.find((w) => w.weapon === weapon)?.kills ?? 0;
    return kills / Math.max(1, props.rec.games) > theirs / Math.max(1, v.games);
  };
  const games = () => Math.max(1, props.rec.games);
  return (
    <>
      <div class="s-tod-key">
        <span>Weapon{props.versus ? " - better per game ratio highlighted" : ""}</span>
        <span>Count (count per game)</span>
      </div>
      <div class="s-tod">
        <For each={all() ? rows() : rows().slice(0, 7)} fallback={<p class="s-tod-empty">No knockouts recorded.</p>}>
          {(r) => (
            <div class="s-tod-row" classList={{ "s-tod-row--top": !!props.versus && better(r.weapon, r.kills) }} title={titleCase(r.weapon)}>
              <span class="s-tod-ico"><span><WeaponIcon weapon={r.weapon} width={50} /><small>{titleCase(r.weapon)}</small></span></span>
              <div class="s-tod-bars">
                <div class="s-tod-bar"><span style={{ width: `${(r.ks / max()) * 78}%` }} />{(r.ks * 100).toFixed(2)}%</div>
                <div class="s-tod-bar"><span style={{ width: `${(r.ds / max()) * 78}%` }} />{(r.ds * 100).toFixed(2)}%</div>
              </div>
              <div class="s-tod-nums">{num(r.kills)} ({perGame(r.kills, games())})<small>{num(r.deaths)} ({perGame(r.deaths, games())})</small></div>
            </div>
          )}
        </For>
        <Show when={rows().length > 7}>
          <div class="s-tod-more"><button class="s-linkbtn" onClick={() => setAll(!all())}>{all() ? "▲ show less" : "▼ show more"}</button></div>
        </Show>
      </div>
    </>
  );
}

function GamerCard(props: { account: AccountState; rec: ServiceRecord }) {
  const r = () => props.rec;
  return (
    <>
      <div class="s-card">
        <Emblem seed={props.account.xuid} size={72} />
        <dl class="s-facts">
          <div><dt>Total Games:</dt><dd>{num(r().games)}</dd></div>
          <div><dt>Total EXP:</dt><dd>{num(r().exp)}</dd></div>
          <div><dt>Best Spree:</dt><dd>{r().bestSpree}</dd></div>
        </dl>
        <span class="s-card-rank" title={RANKS[rankIndex(r().exp)]![1]}><RankIcon rank={rankIndex(r().exp)} exp={r().exp} size={52} /></span>
      </div>
      <dl class="s-statgrid">
        <div><dt>K/D Ratio:</dt><dd>{ratio(r().kills, r().deaths)}</dd></div>
        <div><dt>Win Ratio:</dt><dd>{r().completed ? (r().wins / r().completed).toFixed(2) : "0.00"}</dd></div>
        <div><dt>Kills:</dt><dd>{num(r().kills)}</dd></div>
        <div><dt>Wins:</dt><dd>{num(r().wins)}</dd></div>
        <div><dt>Deaths:</dt><dd>{num(r().deaths)}</dd></div>
        <div><dt>Assists:</dt><dd>{num(r().assists)}</dd></div>
        <div><dt>Games:</dt><dd>{num(r().games)}</dd></div>
        <div><dt>Completed:</dt><dd>{num(r().completed)}</dd></div>
      </dl>
    </>
  );
}

function CompareColumn(props: { account: AccountState; rec: ServiceRecord; versus: ServiceRecord; sort: Sort; onStop?: () => void }) {
  return (
    <div class="s-career-col">
      <section class="s-sec">
        <header>
          <A href={playerHref(props.account.gamertag)}>{props.account.gamertag}</A>
          <Show when={props.onStop} fallback={<SortKey sort={props.sort} />}><button class="s-linkbtn" onClick={() => props.onStop!()}>Stop Comparing</button></Show>
        </header>
        <GamerCard account={props.account} rec={props.rec} />
      </section>
      <section class="s-sec">
        <header><span>Tool of Destruction</span><Derived /></header>
        <ToolOfDestruction rec={props.rec} sort={props.sort} versus={props.versus} />
      </section>
      <section class="s-sec">
        <header><span>Medal Chest</span><Derived /></header>
        <MedalChest rec={props.rec} versus={props.versus} />
      </section>
    </div>
  );
}

const SortKey = (props: { sort: Sort }) => (
  <small>sorted first by <i class="swatch" style={{ background: props.sort === "with" ? "var(--s-gold)" : "var(--s-blue-bar)" }} />then by <i class="swatch" style={{ background: props.sort === "with" ? "var(--s-blue-bar)" : "var(--s-gold)", "margin-right": 0 }} /></small>
);

export function CareerPage() {
  const [search, setSearch] = useSearchParams<{ compare?: string }>();
  const [cmpInput, setCmpInput] = createSignal(search.compare ?? "");
  const [tab, setTab] = createSignal("all");
  const [map, setMap] = createSignal("");
  const [variant, setVariant] = createSignal("");
  const [sort, setSort] = createSignal<Sort>("with");
  const other = () => (search.compare ? accountByGamertag(live.state, search.compare) : undefined);
  const only = (g: GameState) =>
    (tab() === "all" || (tab() === "team") === g.team_play) && (!map() || mapGroupKey(g) === map()) && (!variant() || (g.variant ?? g.mode) === variant());
  return (
    <PlayerFrame which="career">
      {(a, _all, hist) => {
        const rec = createMemo(() => serviceRecord(live.state, a().xuid, only));
        const otherRec = createMemo(() => (other() ? serviceRecord(live.state, other()!.xuid, only) : null));
        const maps = createMemo(() => {
          const m = new Map<string, GameState>();
          // A modded map is its own entry (its map_id), never the retail one.
          for (const h of hist()) { const k = mapGroupKey(h.game); if (!m.has(k)) m.set(k, h.game); }
          return [...m.entries()];
        });
        const variants = createMemo(() => [...new Set(hist().map((h) => h.game.variant ?? h.game.mode))].sort());
        const stop = () => { setCmpInput(""); setSearch({ compare: undefined }); };
        return (
          <div class="s-full">
            <Title class="s-title--bar" back={["Return to Service Record Main", playerHref(a().gamertag)]}>Career Stats</Title>
            <Tabs label="Career stats" active={tab()} onPick={setTab} tabs={[
              { id: "all", label: "Multiplayer" }, { id: "team", label: "Team Games" }, { id: "ffa", label: "Free for All" },
            ]} />
            <div class="s-matte">
              <div class="s-filter">
                <div class="s-filter-selects">
                  <label for="s-f-map">Map</label>
                  <select id="s-f-map" value={map()} onChange={(e) => setMap(e.currentTarget.value)}>
                    <option value="">All Maps</option>
                    <For each={maps()}>{([m, g]) => <option value={m}>{gameMapGroupLabel(g)}</option>}</For>
                  </select>
                  <label for="s-f-var">Gametype</label>
                  <select id="s-f-var" value={variant()} onChange={(e) => setVariant(e.currentTarget.value)}>
                    <option value="">All</option>
                    <For each={variants()}>{(v) => <option value={v}>{titleCase(v)}</option>}</For>
                  </select>
                </div>
                <div class="s-filter-maps" aria-label="Maps">
                  <For each={maps()}>
                    {([m, g]) => <button classList={{ on: map() === m }} onClick={() => setMap(map() === m ? "" : m)} title={gameMapGroupLabel(g)} aria-label={gameMapGroupLabel(g)}><MapThumb game={g} /></button>}
                  </For>
                </div>
              </div>
              <div class="s-sortbar">
                <span>Sort by:</span>
                <button class="s-linkbtn key key-gold" classList={{ on: sort() === "with" }} onClick={() => setSort("with")}><i />Kills With</button>
                <span class="s-dim">|</span>
                <button class="s-linkbtn key key-blue" classList={{ on: sort() === "by" }} onClick={() => setSort("by")}><i />Killed By</button>
                <form onSubmit={(e) => { e.preventDefault(); setSearch({ compare: cmpInput().trim() || undefined }); }}>
                  <Show when={search.compare && !other()}><span class="s-bad">No player called “{search.compare}”.</span></Show>
                  <label for="s-cmp">compare with</label>
                  <input id="s-cmp" value={cmpInput()} onInput={(e) => setCmpInput(e.currentTarget.value)} list="s-cmp-list" />
                  <datalist id="s-cmp-list"><For each={Object.values(live.state.accounts)}>{(x) => <option value={x.gamertag} />}</For></datalist>
                  <button type="submit" class="s-btn">search</button>
                </form>
              </div>
              <Show
                when={other() && otherRec()}
                fallback={
                  <div class="s-career">
                    <div class="s-career-col">
                      <section class="s-sec">
                        <header><A href={playerHref(a().gamertag)}>{a().gamertag}</A><SortKey sort={sort()} /></header>
                        <GamerCard account={a()} rec={rec()} />
                      </section>
                      <section class="s-sec">
                        <header><span>Tool of Destruction</span><Derived note={`From ${rec().killsSeen} recorded knockouts (snapshots carry each game's last 50); derived.`} /></header>
                        <ToolOfDestruction rec={rec()} sort={sort()} />
                      </section>
                    </div>
                    <div class="s-career-col">
                      <section class="s-sec">
                        <header><span>Medal Chest</span><Derived /></header>
                        <MedalChest rec={rec()} />
                      </section>
                    </div>
                  </div>
                }
              >
                <div class="s-career">
                  <CompareColumn account={a()} rec={rec()} versus={otherRec()!} sort={sort()} />
                  <CompareColumn account={other()!} rec={otherRec()!} versus={rec()} sort={sort()} onStop={stop} />
                </div>
              </Show>
              <p class="s-note"><sup>*</sup> Medals, sprees, weapon counts, EXP and rank are derived in your browser from the multiplayer games this page has loaded. Campaign games have <A href={`${playerHref(a().gamertag)}/campaign`}>their own record</A>.</p>
            </div>
          </div>
        );
      }}
    </PlayerFrame>
  );
}
