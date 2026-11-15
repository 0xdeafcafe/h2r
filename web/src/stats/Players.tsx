// Players directory and leaderboards (derived from loaded games).

import { For, Show, createMemo, createSignal } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { rankIndex, serviceRecord } from "../state/derive";
import Emblem from "../components/Emblem";
import RankIcon from "../components/RankIcon";
import { PresenceBadge } from "../components/bits";
import { Crumbs, Derived, Pager, Panel, Strip, Tabs, Title, playerHref } from "./Layout";
import { PeopleFinder, Pills } from "./Side";
import { num, ratio } from "./fmt";

type Board = "kills" | "kd" | "wins" | "spree" | "exp";
const BOARDS: Array<[Board, string, string]> = [
  ["kills", "Most Kills", "Kills"], ["kd", "Best K/D", "K/D Ratio"], ["wins", "Most Wins", "Wins"], ["spree", "Best Spree", "Best Spree"], ["exp", "Top EXP", "EXP"],
];

export default function Players() {
  const [search, setSearch] = useSearchParams<{ q?: string }>();
  const [board, setBoard] = createSignal<Board>("kills");
  const [page, setPage] = createSignal(0);
  const PER = 20;
  const rows = createMemo(() =>
    Object.values(live.state.accounts).map((a) => ({ a, rec: serviceRecord(live.state, a.xuid) })));
  type R = ReturnType<typeof rows>[number];
  const ranked = createMemo(() => {
    const key = board();
    const val = (r: R) =>
      key === "kills" ? r.rec.kills : key === "wins" ? r.rec.wins : key === "spree" ? r.rec.bestSpree : key === "exp" ? r.rec.exp : r.rec.deaths ? r.rec.kills / r.rec.deaths : r.rec.kills;
    return rows().filter((r) => r.rec.games > 0).sort((x, y) => val(y) - val(x) || x.a.gamertag.localeCompare(y.a.gamertag)).slice(0, 13)
      .map((r) => ({ ...r, value: key === "kd" ? val(r).toFixed(2) : num(val(r)) }));
  });
  const directory = createMemo(() => {
    const q = (search.q ?? "").toLowerCase();
    return rows().filter((r) => !q || r.a.gamertag.toLowerCase().includes(q))
      .sort((x, y) => Number(y.a.online) - Number(x.a.online) || x.a.gamertag.localeCompare(y.a.gamertag));
  });
  const online = createMemo(() => rows().filter((r) => r.a.online).slice(0, 8));
  const unit = () => BOARDS.find((b) => b[0] === board())![2];
  const pages = () => Math.max(1, Math.ceil(directory().length / PER));
  const me = () => identity.me();
  return (
    <>
      <Crumbs trail={[["Home", "/"], ["h2 live Online", "/stats"], ["Players"]]} />
      <Strip kind="players" />
      <div class="s-cols s-cols--over">
        <aside class="s-left">
          <Title>Players</Title>
          <Pills label="Players" items={[
            { label: "Leaderboards", href: "#boards" },
            { label: `All Players (${rows().length})`, href: "#directory" },
            me() ? { label: "My Service Record", href: playerHref(me()!.gamertag) } : { label: "Sign Up for h2 live", href: "/dash/signup" },
          ]} />
          <PeopleFinder />
          <Panel title="Online Now" sub="signed in to this server">
            <Show when={online().length} fallback={<p class="s-recent-empty">Nobody is signed in right now.</p>}>
              <ul class="s-mini-list">
                <For each={online()}>
                  {(r) => <li><Emblem seed={r.a.xuid} size={14} /><A href={playerHref(r.a.gamertag)}>{r.a.gamertag}</A><span class="s-dim"><PresenceBadge account={r.a} /></span></li>}
                </For>
              </ul>
            </Show>
          </Panel>
        </aside>
        <div class="s-right">
          <Title class="s-title--cap" help="From the games this page has loaded, not the server's whole history; derived in your browser.">Leaderboards</Title>
          <div id="boards">
            <Tabs label="Leaderboards" active={board()} onPick={(id) => setBoard(id as Board)} tabs={BOARDS.map(([id, label]) => ({ id, label }))} />
            <div class="s-matte" style={{ "padding-bottom": "8px" }}>
              <div class="s-top3">
                <For each={ranked().slice(0, 3)}>
                  {(r, i) => (
                    <A href={playerHref(r.a.gamertag)}>
                      <Emblem seed={r.a.xuid} size={48} />
                      <span><b>{r.a.gamertag}</b>{unit()}: <b style={{ display: "inline", "font-size": "11px" }}>{r.value}</b><br />{r.rec.rank}, {num(r.rec.games)} games</span>
                      <em>{i() + 1}</em>
                    </A>
                  )}
                </For>
              </div>
              <div class="s-grid-wrap" style={{ margin: "8px 8px 0" }}>
                <table class="s-grid s-grid--roomy">
                  <thead><tr><th style={{ width: "34px" }}>#</th><th>Player</th><th class="hide-s">Rank</th><th class="num">{unit()} <Derived>{""}</Derived></th><th class="num">Games</th></tr></thead>
                  <tbody>
                    <For each={ranked().slice(3)} fallback={<tr><td colspan="5" class="s-dim">{ranked().length ? "That's everyone with a game on record." : "No games played yet."}</td></tr>}>
                      {(r, i) => (
                        <tr>
                          <td>{i() + 4}</td>
                          <td><span class="s-who"><Emblem seed={r.a.xuid} size={16} /><A href={playerHref(r.a.gamertag)}>{r.a.gamertag}</A></span></td>
                          <td class="hide-s"><span class="s-who"><RankIcon rank={rankIndex(r.rec.exp)} exp={r.rec.exp} size={16} />{r.rec.rank}</span></td>
                          <td class="num">{r.value}</td>
                          <td class="num">{num(r.rec.games)}</td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </div>
          </div>
          <Title class="s-title--tabs">All Players</Title>
          <div id="directory" class="s-matte" style={{ padding: "8px" }}>
            <form class="s-toolbar" onSubmit={(e) => e.preventDefault()}>
              <label for="s-dir">find a gamertag</label>
              <input id="s-dir" value={search.q ?? ""} onInput={(e) => { setPage(0); setSearch({ q: e.currentTarget.value || undefined }, { replace: true }); }} />
              <span class="s-dim" style={{ "margin-left": "auto", color: "#9d9d9d" }}>online players first</span>
            </form>
            <div class="s-grid-wrap">
              <div class="s-grid-pager"><Pager page={page()} pages={pages()} total={directory().length} per={PER} onPage={setPage} /></div>
              <table class="s-grid s-grid--roomy">
                <thead><tr><th>Player</th><th class="hide-s">Rank</th><th>Status</th><th class="num">Games</th><th class="num">Kills</th><th class="num">K/D</th></tr></thead>
                <tbody>
                  <For each={directory().slice(page() * PER, page() * PER + PER)} fallback={<tr><td colspan="6" class="s-dim">No matching gamertags.</td></tr>}>
                    {(r) => (
                      <tr>
                        <td><span class="s-who"><Emblem seed={r.a.xuid} size={16} /><A href={playerHref(r.a.gamertag)}>{r.a.gamertag}</A></span></td>
                        <td class="hide-s"><span class="s-who"><RankIcon rank={rankIndex(r.rec.exp)} exp={r.rec.exp} size={16} />{r.rec.rank}</span></td>
                        <td><PresenceBadge account={r.a} /></td>
                        <td class="num">{num(r.rec.games)}</td>
                        <td class="num">{num(r.rec.kills)}</td>
                        <td class="num">{r.rec.games ? ratio(r.rec.kills, r.rec.deaths) : "–"}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
