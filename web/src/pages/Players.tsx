import { For, Show, createMemo, createSignal, createEffect, on } from "solid-js";
import { A, useParams } from "@solidjs/router";
import { live } from "../state/live";
import { accountByGamertag, gamesFor, playerStats, standings } from "../state/reducer";
import { Avatar, Empty, PresenceBadge, presenceKind } from "../components/bits";
import { ago, clock, titleCase } from "../lib/format";
import { elapsedSeconds, teamName } from "../lib/game";
import { now } from "../lib/now";

export function Players() {
  const [q, setQ] = createSignal("");
  const [onlyOnline, setOnlyOnline] = createSignal(false);
  const list = createMemo(() => {
    const needle = q().trim().toLowerCase();
    return Object.values(live.state.accounts)
      .filter((a) => (!onlyOnline() || a.online) && (!needle || a.gamertag.toLowerCase().includes(needle)))
      .sort((a, b) => Number(b.online) - Number(a.online) || a.gamertag.localeCompare(b.gamertag));
  });
  return (
    <div class="page players-page">
      <h1>Players</h1>
      <p class="muted">Everyone who has ever signed up on this server. {Object.values(live.state.accounts).filter((a) => a.online).length} online right now.</p>
      <div class="toolbar">
        <input placeholder="Find a gamertag…" value={q()} onInput={(e) => setQ(e.currentTarget.value)} aria-label="Search players" />
        <label class="check"><input type="checkbox" checked={onlyOnline()} onChange={(e) => setOnlyOnline(e.currentTarget.checked)} /> online only</label>
      </div>
      <Show when={list().length} fallback={<Empty title={q() ? "Nobody by that name" : "No players yet"} mood="sleepy">{q() ? "Try another spelling?" : "Create an account with h2 live account create."}</Empty>}>
        <ul class="player-grid">
          <For each={list()}>
            {(a) => (
              <li>
                <A href={`/players/${encodeURIComponent(a.gamertag)}`} class="player-card" classList={{ [`player-card--${presenceKind(a)}`]: true }}>
                  <Avatar name={a.gamertag} size="l" online={a.online} />
                  <div>
                    <div class="player-card__name">{a.gamertag}</div>
                    <PresenceBadge account={a} />
                    <div class="muted small">{a.online ? `since ${ago(a.presence?.since, now())}` : a.last_seen ? `seen ${ago(a.last_seen, now())}` : `joined ${ago(a.created_at, now())}`}</div>
                  </div>
                </A>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </div>
  );
}

export function Player() {
  const params = useParams<{ tag: string }>();
  const tag = () => decodeURIComponent(params.tag);
  const acct = () => accountByGamertag(live.state, tag());
  const stats = createMemo(() => (acct() ? playerStats(live.state, acct()!.xuid) : null));
  const games = createMemo(() => (acct() ? gamesFor(live.state, acct()!.xuid).slice(0, 25) : []));
  const [restRecent, setRestRecent] = createSignal<number | null>(null);
  // The profile's recent games may be older than the snapshot: load their
  // scoreboards so the stats include them.
  createEffect(on(() => [tag(), live.loadMode()] as const, ([t, mode]) => {
    if (mode === "snapshot") return;
    live.api().account(t).then((a) => {
      setRestRecent(a.recent_games?.length ?? null);
      for (const g of (a.recent_games ?? []).slice(0, 20)) void live.ensureGame(g.game_id);
    }, () => setRestRecent(null));
  }));

  return (
    <div class="page player-page">
      <Show
        when={acct()}
        fallback={
          <Show when={live.loadMode() !== "snapshot"} fallback={<div class="loading">Looking them up…</div>}>
            <Empty title={`No one called "${tag()}" here`} mood="wow"><A href="/players">See everyone</A></Empty>
          </Show>
        }
      >
        {(a) => (
          <>
            <header class="profile">
              <Avatar name={a().gamertag} size="l" online={a().online} />
              <div>
                <h1>{a().gamertag}</h1>
                <PresenceBadge account={a()} />
                <div class="muted small">joined {ago(a().created_at, now())} · xuid <code>{a().xuid}</code></div>
              </div>
            </header>
            <div class="stat-row">
              <Stat label="games" value={stats()!.games} />
              <Stat label="knockouts" value={stats()!.kills} />
              <Stat label="times knocked out" value={stats()!.deaths} />
              <Stat label="k/d" value={stats()!.deaths ? (stats()!.kills / stats()!.deaths).toFixed(2) : String(stats()!.kills)} />
              <Stat label="wins" value={stats()!.wins} />
            </div>
            <h2 class="section__title">Recent games</h2>
            <Show when={games().length} fallback={<Empty title="Hasn't played yet" mood="sleepy">First game's the best game.</Empty>}>
              <table class="table">
                <thead><tr><th>map</th><th>mode</th><th>team</th><th class="num">score</th><th class="num">K</th><th class="num">D</th><th>length</th><th>when</th></tr></thead>
                <tbody>
                  <For each={games()}>
                    {(g) => {
                      const me = () => standings(g).find((s) => s.xuid === a().xuid);
                      return (
                        <tr>
                          <td><A href={`/games/${g.game_id}`}>{titleCase(g.map)}</A> <Show when={g.status === "live"}><span class="live-pill live-pill--inline">live</span></Show></td>
                          <td>{titleCase(g.variant ?? g.mode)}</td>
                          <td>{g.team_play && me() ? teamName(me()!.team) : "–"}</td>
                          <td class="num">{me()?.score.score ?? 0}</td>
                          <td class="num">{me()?.score.kills ?? 0}</td>
                          <td class="num">{me()?.score.deaths ?? 0}</td>
                          <td>{clock(elapsedSeconds(g, now()))}</td>
                          <td class="muted">{ago(g.ended_at ?? g.started_at, now())}</td>
                        </tr>
                      );
                    }}
                  </For>
                </tbody>
              </table>
            </Show>
            <Show when={restRecent() !== null}><p class="muted small">Server reports {restRecent()} recent games.</p></Show>
          </>
        )}
      </Show>
    </div>
  );
}

function Stat(props: { label: string; value: number | string }) {
  return (
    <div class="stat">
      <div class="stat__value">{props.value}</div>
      <div class="stat__label">{props.label}</div>
    </div>
  );
}
