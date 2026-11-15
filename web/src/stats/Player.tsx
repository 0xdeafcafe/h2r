// Player pages: service record, game history and career stats, all derived
// from the events the client has seen.

import { For, Show, createEffect, createMemo, createSignal, on, untrack, type JSX } from "solid-js";
import { A, useParams, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { accountByGamertag, type AccountState } from "../state/reducer";
import {
  MEDALS, gameHistory, ordinal, playedWith, serviceRecord, type HistoryRow, type ServiceRecord, gameMedals, countMedals,
  type MedalId,
} from "../state/derive";
import Emblem from "../components/Emblem";
import MedalIcon from "../components/MedalIcon";
import { PresenceBadge } from "../components/bits";
import { Crumbs, Derived, Pager, Panel } from "./Layout";
import { ago, clock, titleCase } from "../lib/format";
import { elapsedSeconds, teamName } from "../lib/game";
import { now } from "../lib/now";
import { serviceTag } from "../state/derive";

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
      <Panel title="Player not found"><p class="s-pad">No one called “{props.tag}” has an account on this server. <A href="/stats/players">Browse players</A></p></Panel>
    </Show>
  );
}

export function Banner(props: { account: AccountState; rec: ServiceRecord }) {
  const topMedals = () => [...props.rec.medals.entries()].sort((a, b) => b[1] - a[1]).slice(0, 5);
  return (
    <div class="s-banner">
      <Emblem seed={props.account.xuid} size={64} class="s-banner-emblem" />
      <div class="s-banner-id">
        <div class="s-banner-tag">{props.account.gamertag}</div>
        <div>Service Tag: <b>{serviceTag(props.account.gamertag, props.account.xuid)}</b></div>
        <div><Derived>{props.rec.rank}</Derived> · <PresenceBadge account={props.account} /></div>
      </div>
      <div class="s-banner-nums">
        <div>Best Spree: <b>{props.rec.bestSpree}</b></div>
        <div>Total EXP: <b><Derived note="One point per completed game, two more per win; derived in your browser.">{props.rec.exp}</Derived></b></div>
        <div>Games: <b>{props.rec.games}</b></div>
      </div>
      <div class="s-banner-medals">
        <For each={topMedals()}>{([id, n]) => <span class="s-medal-mini"><MedalIcon id={id} size={30} /><small>{n}</small></span>}</For>
      </div>
    </div>
  );
}

function PlayerFrame(props: { which: "record" | "history" | "career"; children: (a: () => AccountState, rec: () => ServiceRecord, hist: () => HistoryRow[]) => JSX.Element }) {
  const { tag, acct } = useAccount();
  const rec = createMemo(() => (acct() ? serviceRecord(live.state, acct()!.xuid) : null));
  const hist = createMemo(() => (acct() ? gameHistory(live.state, acct()!.xuid) : []));
  const titles = { record: "Service Record", history: "Game History", career: "Career Stats" };
  return (
    <Show when={acct()} fallback={<><Crumbs trail={[["Home", "/stats"], ["Players", "/stats/players"], [tag()]]} /><NotFound tag={tag()} /></>}>
      {(a) => {
        const base = `/stats/players/${encodeURIComponent(a().gamertag)}`;
        return (
          <>
            <Crumbs trail={[["Home", "/stats"], ["Players", "/stats/players"], [a().gamertag, base], ...(props.which === "record" ? [] : [[titles[props.which]] as [string]])]} />
            <Banner account={a()} rec={rec()!} />
            <div class="s-two">
              <aside class="s-left">
                <h2 class="s-h2">{a().gamertag}</h2>
                <nav class="s-pills" aria-label="Player">
                  <A href={base} end>Service Record</A>
                  <A href={`${base}/history`}>Game History ({hist().length})</A>
                  <A href={`${base}/career`}>Career Stats</A>
                </nav>
                <PlayedWith account={a()} />
              </aside>
              <div class="s-right">
                <h1 class="s-h1">{titles[props.which]} <A href={base} class="s-h1-link">{props.which === "record" ? "" : "Return to Service Record"}</A></h1>
                {untrack(() => props.children(a, () => rec()!, hist))}
              </div>
            </div>
          </>
        );
      }}
    </Show>
  );
}

function PlayedWith(props: { account: AccountState }) {
  const list = createMemo(() => playedWith(live.state, props.account.xuid).slice(0, 8));
  return (
    <Panel title="Played With" class="s-side-panel">
      <Show when={list().length} fallback={<p class="s-pad s-dim">No shared games yet.</p>}>
        <ul class="s-mini-list">
          <For each={list()}>
            {(f) => {
              const a = () => live.state.accounts[f.xuid];
              return (
                <li>
                  <Show when={a()}>{(x) => <A href={`/stats/players/${encodeURIComponent(x().gamertag)}`}>{x().gamertag}</A>}</Show>
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

function kd(r: { kills: number; deaths: number }): string {
  return r.deaths ? (r.kills / r.deaths).toFixed(2) : r.kills ? `${r.kills}.00` : "0.00";
}

export function ServiceRecordPage() {
  return (
    <PlayerFrame which="record">
      {(a, rec, hist) => (
        <>
          <Panel title="Player Snapshot" right={<Derived />}>
            <div class="s-snapshot">
              <Emblem seed={a().xuid} size={110} />
              <dl class="s-dl">
                <dt>Total Games</dt><dd>{rec().games}</dd>
                <dt>Completed</dt><dd>{rec().completed}</dd>
                <dt>Wins</dt><dd>{rec().wins}</dd>
                <dt>Kills</dt><dd>{rec().kills}</dd>
                <dt>Deaths</dt><dd>{rec().deaths}</dd>
                <dt>Assists</dt><dd>{rec().assists}</dd>
              </dl>
              <dl class="s-dl">
                <dt>K/D Ratio</dt><dd>{kd(rec())}</dd>
                <dt>Best Spree</dt><dd>{rec().bestSpree}</dd>
                <dt>Rank</dt><dd>{rec().rank}</dd>
                <dt>Total EXP</dt><dd>{rec().exp}</dd>
                <dt>Player Since</dt><dd>{new Date(a().created_at).toLocaleDateString()}</dd>
                <dt>Last Played</dt><dd>{rec().lastPlayed ? ago(rec().lastPlayed, now()) : "never"}</dd>
              </dl>
            </div>
          </Panel>
          <Panel title="Recent Games" right={<A href={`/stats/players/${encodeURIComponent(a().gamertag)}/history`}>See all</A>}>
            <HistoryTable rows={hist().slice(0, 6)} />
          </Panel>
          <Panel title="Top Medals" right={<Derived />}>
            <MedalChest counts={rec().medals} compact />
          </Panel>
        </>
      )}
    </PlayerFrame>
  );
}

function HistoryTable(props: { rows: HistoryRow[]; selected?: string; onSelect?: (id: string) => void }) {
  return (
    <table class="s-table">
      <thead><tr><th>Game</th><th>Date</th><th>Map</th><th>Playlist</th><th>Place</th><th /></tr></thead>
      <tbody>
        <For each={props.rows} fallback={<tr><td colspan="6" class="s-dim">Your first chapter is still ahead.</td></tr>}>
          {(r) => (
            <tr classList={{ sel: props.selected === r.game.game_id }} onClick={() => props.onSelect?.(r.game.game_id)}>
              <td><A href={`/stats/games/${r.game.game_id}`}>{titleCase(r.game.variant ?? r.game.mode)}</A></td>
              <td>{new Date(r.game.started_at).toLocaleString()}</td>
              <td>{titleCase(r.game.map)}</td>
              <td>{r.game.team_play ? "Team Game" : "Free for All"}</td>
              <td>{r.game.status === "live" ? <span class="s-live-tag">LIVE</span> : r.place ? ordinal(r.place) : "–"}</td>
              <td>
                <Show when={props.onSelect} fallback={<A href={`/stats/games/${r.game.game_id}`}>details</A>}>
                  <button class="s-linkbtn" onClick={(e) => { e.stopPropagation(); props.onSelect!(r.game.game_id); }}>preview</button>
                </Show>
              </td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}

export function HistoryPage() {
  const PER = 25;
  const [page, setPage] = createSignal(0);
  const [sel, setSel] = createSignal<string | null>(null);
  return (
    <PlayerFrame which="history">
      {(a, _rec, hist) => {
        const pages = () => Math.max(1, Math.ceil(hist().length / PER));
        const rows = () => hist().slice(page() * PER, page() * PER + PER);
        const chosen = () => hist().find((r) => r.game.game_id === sel()) ?? hist()[0];
        return (
          <>
            <div class="s-tabs"><span class="on">Multiplayer</span></div>
            <div class="s-history">
              <Panel title={<Pager page={page()} pages={pages()} total={hist().length} per={PER} onPage={setPage} />}>
                <HistoryTable rows={rows()} selected={chosen()?.game.game_id} onSelect={setSel} />
                <div class="s-panel-foot"><Pager page={page()} pages={pages()} total={hist().length} per={PER} onPage={setPage} /></div>
              </Panel>
              <Show when={chosen()}>{(r) => <Preview row={r()} gamertag={a().gamertag} />}</Show>
            </div>
          </>
        );
      }}
    </PlayerFrame>
  );
}

function Preview(props: { row: HistoryRow; gamertag: string }) {
  const g = () => props.row.game;
  const medals = createMemo(() => [...countMedals(gameMedals(g().kills), new Set(props.row.slots)).entries()].slice(0, 6));
  const victims = createMemo(() => {
    const m = new Map<number, number>();
    for (const k of g().kills) if (k.killer !== null && props.row.slots.includes(k.killer) && k.kind === "kill") m.set(k.victim, (m.get(k.victim) ?? 0) + 1);
    const top = [...m.entries()].sort((a, b) => b[1] - a[1])[0];
    return top ? g().players[top[0]]?.name : null;
  });
  return (
    <Panel title="Game Preview" class="s-preview">
      <div class="s-preview-map"><A href={`/stats/games/${g().game_id}`}>{titleCase(g().variant ?? g().mode)}</A> on {titleCase(g().map)}</div>
      <p class="s-pad s-dim">
        Playlist: {g().team_play ? "Team Game" : "Free for All"}<br />
        {new Date(g().started_at).toLocaleString()}<br />
        Length: {clock(elapsedSeconds(g(), now()))}
      </p>
      <p class="s-pad">
        {props.gamertag} {g().status === "live" ? "is playing now" : props.row.place ? `placed ${ordinal(props.row.place)}` : "played"}
        {props.row.team !== null ? ` on ${teamName(props.row.team)} team` : ""}.
      </p>
      <dl class="s-dl s-pad">
        <dt>Score</dt><dd>{props.row.score}</dd>
        <dt>Kills / Deaths</dt><dd>{props.row.kills} / {props.row.deaths}</dd>
        <dt>Best Spree</dt><dd>{props.row.bestSpree}</dd>
        <dt>Most Knocked Out</dt><dd>{victims() ?? "–"}</dd>
      </dl>
      <div class="s-sub-head">Top Medals of {medals().length} Earned <Derived /></div>
      <div class="s-medal-row">
        <For each={medals()} fallback={<span class="s-dim s-pad">None this game.</span>}>{([id, n]) => <span class="s-medal-mini"><MedalIcon id={id} size={34} /><small>{n}</small></span>}</For>
      </div>
      <div class="s-pad"><A href={`/stats/games/${g().game_id}`}>View the carnage report ›</A></div>
    </Panel>
  );
}

export function MedalChest(props: { counts: Map<MedalId, number>; compact?: boolean }) {
  const shown = () => (props.compact ? MEDALS.filter((m) => props.counts.get(m.id)) : MEDALS);
  return (
    <div class="s-chest" classList={{ "s-chest--compact": !!props.compact }}>
      <For each={shown()} fallback={<p class="s-dim s-pad">No medals yet.</p>}>
        {(m) => {
          const n = () => props.counts.get(m.id) ?? 0;
          return (
            <div class="s-chest-cell" classList={{ empty: !n() }} title={`${m.name}: ${m.blurb} (derived)`}>
              <MedalIcon id={m.id} size={32} dim={!n()} />
              <div><b>{n()}</b><small>{m.name}</small></div>
            </div>
          );
        }}
      </For>
    </div>
  );
}

function CareerColumn(props: { account: AccountState; rec: ServiceRecord }) {
  const max = () => Math.max(1, ...props.rec.weapons.map((w) => w.kills));
  return (
    <div class="s-career-col">
      <Panel title={<A href={`/stats/players/${encodeURIComponent(props.account.gamertag)}`}>{props.account.gamertag}</A>} right={<Derived />}>
        <div class="s-career-id">
          <Emblem seed={props.account.xuid} size={72} />
          <dl class="s-dl">
            <dt>Total Games</dt><dd>{props.rec.games}</dd>
            <dt>Total EXP</dt><dd>{props.rec.exp}</dd>
            <dt>Rank</dt><dd>{props.rec.rank}</dd>
          </dl>
        </div>
        <div class="s-kd-boxes">
          <dl class="s-dl"><dt>K/D Ratio</dt><dd>{kd(props.rec)}</dd><dt>Kills</dt><dd>{props.rec.kills}</dd><dt>Deaths</dt><dd>{props.rec.deaths}</dd><dt>Games</dt><dd>{props.rec.games}</dd></dl>
          <dl class="s-dl"><dt>Wins</dt><dd>{props.rec.wins}</dd><dt>Assists</dt><dd>{props.rec.assists}</dd><dt>Best Spree</dt><dd>{props.rec.bestSpree}</dd><dt>Win Rate</dt><dd>{props.rec.completed ? Math.round((props.rec.wins / props.rec.completed) * 100) + "%" : "–"}</dd></dl>
        </div>
      </Panel>
      <Panel title="Tool of Destruction" right={<Derived note={`Kills by weapon from ${props.rec.killsSeen} recorded knockouts (snapshots carry each game's last 50); derived.`} />}>
        <table class="s-tod">
          <thead><tr><th>Weapon</th><th class="num">Kills (share)</th></tr></thead>
          <tbody>
            <For each={props.rec.weapons} fallback={<tr><td colspan="2" class="s-dim">No knockouts recorded.</td></tr>}>
              {(w) => (
                <tr>
                  <td>
                    <span class="s-tod-name">{titleCase(w.weapon)}</span>
                    <span class="s-tod-bar"><span style={{ width: `${(w.kills / max()) * 100}%` }} /></span>
                  </td>
                  <td class="num">{w.kills} ({(w.share * 100).toFixed(1)}%)</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Panel>
      <Panel title="Medal Chest" right={<Derived />}>
        <MedalChest counts={props.rec.medals} />
      </Panel>
    </div>
  );
}

export function CareerPage() {
  const [search, setSearch] = useSearchParams<{ compare?: string }>();
  const [cmpInput, setCmpInput] = createSignal(search.compare ?? "");
  const other = () => (search.compare ? accountByGamertag(live.state, search.compare) : undefined);
  const otherRec = createMemo(() => (other() ? serviceRecord(live.state, other()!.xuid) : null));
  return (
    <PlayerFrame which="career">
      {(a, rec) => (
        <>
          <div class="s-tabs"><span class="on">Matchmaking Stats</span></div>
          <form class="s-compare" onSubmit={(e) => { e.preventDefault(); setSearch({ compare: cmpInput().trim() || undefined }); }}>
            <label for="s-cmp">compare with</label>
            <input id="s-cmp" value={cmpInput()} onInput={(e) => setCmpInput(e.currentTarget.value)} placeholder="gamertag" list="s-cmp-list" />
            <datalist id="s-cmp-list"><For each={Object.values(live.state.accounts)}>{(x) => <option value={x.gamertag} />}</For></datalist>
            <button type="submit">search</button>
            <Show when={search.compare && !other()}><span class="s-bad"> No player called “{search.compare}”.</span></Show>
            <Show when={other()}><button type="button" class="s-linkbtn" onClick={() => { setCmpInput(""); setSearch({ compare: undefined }); }}>stop comparing</button></Show>
          </form>
          <div class="s-career" classList={{ "s-career--two": !!other() }}>
            <CareerColumn account={a()} rec={rec()} />
            <Show when={other() && otherRec()}><CareerColumn account={other()!} rec={otherRec()!} /></Show>
          </div>
        </>
      )}
    </PlayerFrame>
  );
}
