// Players directory and leaderboards (derived from loaded games).

import { For, Show, createMemo, createSignal } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { serviceRecord } from "../state/derive";
import Emblem from "../components/Emblem";
import { PresenceBadge } from "../components/bits";
import { Crumbs, Derived, Pager, Panel } from "./Layout";

type Board = "kills" | "kd" | "wins" | "spree";
const BOARDS: Array<[Board, string]> = [["kills", "Most Kills"], ["kd", "Best K/D"], ["wins", "Most Wins"], ["spree", "Best Spree"]];

export default function Players() {
  const [search, setSearch] = useSearchParams<{ q?: string }>();
  const [board, setBoard] = createSignal<Board>("kills");
  const [page, setPage] = createSignal(0);
  const PER = 20;
  const rows = createMemo(() =>
    Object.values(live.state.accounts).map((a) => ({ a, rec: serviceRecord(live.state, a.xuid) })));
  const ranked = createMemo(() => {
    const key = board();
    const val = (r: (typeof rows extends () => infer T ? T : never)[number]) =>
      key === "kills" ? r.rec.kills : key === "wins" ? r.rec.wins : key === "spree" ? r.rec.bestSpree : r.rec.deaths ? r.rec.kills / r.rec.deaths : r.rec.kills;
    return rows().filter((r) => r.rec.games > 0).sort((x, y) => val(y) - val(x) || x.a.gamertag.localeCompare(y.a.gamertag)).slice(0, 10)
      .map((r) => ({ ...r, value: key === "kd" ? val(r).toFixed(2) : String(val(r)) }));
  });
  const directory = createMemo(() => {
    const q = (search.q ?? "").toLowerCase();
    return rows().filter((r) => !q || r.a.gamertag.toLowerCase().includes(q))
      .sort((x, y) => Number(y.a.online) - Number(x.a.online) || x.a.gamertag.localeCompare(y.a.gamertag));
  });
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], ["Players"]]} />
      <h1 class="s-h1">Players</h1>
      <div class="s-two s-two--even">
        <Panel title="Leaderboards" right={<Derived note="From the games this page has loaded, not the server's whole history; derived." />}>
          <div class="s-tabs s-tabs--inset">
            <For each={BOARDS}>{([id, label]) => <button classList={{ on: board() === id }} onClick={() => setBoard(id)}>{label}</button>}</For>
          </div>
          <table class="s-table">
            <thead><tr><th>#</th><th>Player</th><th class="num">{BOARDS.find((b) => b[0] === board())![1].replace(/^(Most|Best) /, "")}</th><th class="num">Games</th></tr></thead>
            <tbody>
              <For each={ranked()} fallback={<tr><td colspan="4" class="s-dim">No games played yet.</td></tr>}>
                {(r, i) => (
                  <tr>
                    <td>{i() + 1}</td>
                    <td class="s-player-cell"><Emblem seed={r.a.xuid} size={18} /><A href={`/stats/players/${encodeURIComponent(r.a.gamertag)}`}>{r.a.gamertag}</A></td>
                    <td class="num">{r.value}</td>
                    <td class="num">{r.rec.games}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Panel>
        <Panel title={<Pager page={page()} pages={Math.max(1, Math.ceil(directory().length / PER))} total={directory().length} per={PER} onPage={setPage} />}>
          <form class="s-finder s-finder--row" onSubmit={(e) => e.preventDefault()}>
            <label for="s-dir">Find a gamertag</label>
            <input id="s-dir" value={search.q ?? ""} onInput={(e) => { setPage(0); setSearch({ q: e.currentTarget.value || undefined }, { replace: true }); }} />
          </form>
          <table class="s-table">
            <thead><tr><th>Player</th><th>Status</th><th class="num">Games</th><th class="num">K/D</th></tr></thead>
            <tbody>
              <For each={directory().slice(page() * PER, page() * PER + PER)} fallback={<tr><td colspan="4" class="s-dim">No matching gamertags.</td></tr>}>
                {(r) => (
                  <tr>
                    <td class="s-player-cell"><Emblem seed={r.a.xuid} size={18} /><A href={`/stats/players/${encodeURIComponent(r.a.gamertag)}`}>{r.a.gamertag}</A></td>
                    <td><PresenceBadge account={r.a} /></td>
                    <td class="num">{r.rec.games}</td>
                    <td class="num">{r.rec.deaths ? (r.rec.kills / r.rec.deaths).toFixed(2) : <Show when={r.rec.kills} fallback="–">{r.rec.kills.toFixed(2)}</Show>}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Panel>
      </div>
    </>
  );
}
