// The player-report queue (docs/live/live-admin.md §8), on the dashboard's
// server screens at /dash/server/reports: complaints players filed about
// each other from the game, newest first, each with the reported player's
// record, and Note or Dismiss on each. A note closes the report and stays
// on the player's record; Ban (./Bans.tsx) bans the player for it. It is one self-contained component over
// ../admin/reports.ts, laid out plainly inside a plate, so it can move to
// another admin layout as it is. Admins only: the server refuses everyone
// else, and the component says so without asking.

import { For, Show, createEffect, createSignal, on } from "solid-js";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { adminState } from "../admin/state";
import { ago } from "../lib/format";
import { BanForm } from "./Bans";
import { KIND_LABELS, fetchReports, recordFor, reviewProblem, reviewReport, type PlayerReport, type ReportList, type ReportState, type ReviewAction } from "../admin/reports";

const STATES: { id: ReportState; label: string }[] = [
  { id: "open", label: "open" },
  { id: "closed", label: "closed" },
  { id: "all", label: "everything" },
];

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

export function ReportsQueue() {
  const [state, setState] = createSignal<ReportState>("open");
  const [reports, setReports] = createSignal<ReportList | null>(null);
  const [notes, setNotes] = createSignal<Record<number, string>>({});
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const admin = adminState.isMeAdmin;

  const ctx = () => {
    const me = identity.me(), wk = live.wellKnown();
    return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
  };

  let seq = 0;
  async function load() {
    const c = ctx();
    if (!c || admin() !== true) return;
    const n = ++seq;
    try {
      const got = await fetchReports(c.api, c.aud, c.me, state());
      if (n === seq) { setReports(got); setErr(null); }
    } catch (x) {
      if (n === seq) setErr((x as Error).message);
    }
  }
  createEffect(on(() => [admin(), state(), live.server().base, identity.me()?.xuid], () => void load()));

  async function review(r: PlayerReport, action: ReviewAction) {
    const c = ctx();
    if (!c || busy()) return;
    const note = notes()[r.id] ?? "";
    const problem = reviewProblem(action, note);
    if (problem) { setErr(problem); return; }
    setBusy(true); setErr(null);
    try {
      await reviewReport(c.api, c.aud, c.me, r.id, action, note);
      setNotes((all) => { const { [r.id]: _, ...rest } = all; return rest; });
      await load();
    } catch (x) {
      setErr(`couldn't ${action}: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }

  return (
    <div class="dx-reports">
      <Show when={admin() === true} fallback={<p class="dx-note">{admin() === null ? "Checking whether you are an admin…" : "Only an admin reads player reports."}</p>}>
        <div class="dx-reports-states" role="tablist" aria-label="which reports">
          <For each={STATES}>
            {(s) => (
              <button type="button" role="tab" class="dx-forget" aria-selected={state() === s.id} classList={{ on: state() === s.id }} onClick={() => { setReports(null); setState(s.id); }}>
                {s.label}
              </button>
            )}
          </For>
          <button type="button" class="dx-forget" disabled={busy()} onClick={() => void load()}>refresh</button>
        </div>
        <Show when={reports()?.items.length} fallback={<p class="dx-note">{reports() ? (state() === "open" ? "Nothing to review." : "No reports.") : "Loading…"}</p>}>
          <ul class="dx-reports-list">
            <For each={reports()!.items}>
              {(r) => {
                const rec = () => recordFor(reports()!, r.target.xuid);
                const note = () => notes()[r.id] ?? "";
                return (
                  <li classList={{ "dx-report--closed": !!r.review, "dx-report--good": !r.negative }}>
                    <p class="dx-report-head">
                      <b>{r.target.gamertag}</b> {KIND_LABELS[r.kind]}
                      <span> by {r.by.gamertag}, {r.at ? ago(r.at) : "at an unknown time"}</span>
                      <Show when={r.game_id}>{(g) => <> · <a href={`/stats/games/${encodeURIComponent(g())}`}>the game</a></>}</Show>
                    </p>
                    <Show when={r.reason}><p class="dx-report-reason">{r.reason}</p></Show>
                    <p class="dx-note">
                      {plural(rec().complaints, "complaint")} ({rec().open} open), {plural(rec().commendations, "commendation")}
                      <For each={rec().notes}>{(n) => <><br />note, {ago(n.at)}: {n.note}</>}</For>
                    </p>
                    <Show
                      when={!r.review}
                      fallback={
                        <p class="dx-note">
                          {r.review!.action === "note" ? "noted" : "dismissed"} {r.review!.by ? `by ${r.review!.by}` : "by the operator"} {ago(r.review!.at)}
                          {r.review!.note ? `: ${r.review!.note}` : ""}
                        </p>
                      }
                    >
                      <Show when={r.negative}>
                        <form class="dx-report-form" onSubmit={(e) => { e.preventDefault(); void review(r, "note"); }}>
                          <input
                            aria-label={`note on ${r.target.gamertag}`} placeholder="note for the player's record" maxLength={500} value={note()}
                            onInput={(e) => { const v = e.currentTarget.value; setNotes((all) => ({ ...all, [r.id]: v })); }}
                          />
                          <button type="submit" class="dx-forget" disabled={busy() || !note().trim()}>note</button>
                          <button type="button" class="dx-forget" disabled={busy()} onClick={() => void review(r, "dismiss")}>dismiss</button>
                        </form>
                        <BanForm target={r.target} report={r.id} />
                      </Show>
                    </Show>
                  </li>
                );
              }}
            </For>
          </ul>
        </Show>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
      </Show>
    </div>
  );
}
