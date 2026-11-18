// Bans on the dashboard (docs/live/live-admin.md "Bans"), over
// ../admin/bans.ts:
//   - BansList, at /dash/server/bans: the bans in force (or every ban),
//     newest first, with lift on each;
//   - BanForm: ban one player, for a length picked from a short list, with
//     the reason the player is told; on each report in the queue
//     (./Reports.tsx, which it answers) and on a player's screen for admins;
//   - BanNotice: the signed-in player's own ban, on their screen.
// Admins only, apart from the notice; the server refuses everyone else.

import { For, Show, createEffect, createSignal, on } from "solid-js";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { adminState } from "../admin/state";
import { ago } from "../lib/format";
import { BAN_LENGTHS, banAccount, banProblem, fetchBan, fetchBans, liftBan, untilText, type Ban, type BanList, type BanListState, type BanNotice as Notice } from "../admin/bans";

const ctx = () => {
  const me = identity.me(), wk = live.wellKnown();
  return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
};

const STATE_WORDS: Record<Ban["state"], string> = { active: "in force", expired: "ended", lifted: "lifted", replaced: "replaced by a newer ban" };

export function BansList() {
  const [state, setState] = createSignal<BanListState>("active");
  const [bans, setBans] = createSignal<BanList | null>(null);
  const [notes, setNotes] = createSignal<Record<number, string>>({});
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const admin = adminState.isMeAdmin;

  let seq = 0;
  async function load() {
    const c = ctx();
    if (!c || admin() !== true) return;
    const n = ++seq;
    try {
      const got = await fetchBans(c.api, c.aud, c.me, state());
      if (n === seq) { setBans(got); setErr(null); }
    } catch (x) {
      if (n === seq) setErr((x as Error).message);
    }
  }
  createEffect(on(() => [admin(), state(), live.server().base, identity.me()?.xuid], () => void load()));

  async function lift(b: Ban) {
    const c = ctx();
    if (!c || busy()) return;
    setBusy(true); setErr(null);
    try {
      await liftBan(c.api, c.aud, c.me, b.target.xuid, notes()[b.id] ?? "");
      setNotes((all) => { const { [b.id]: _, ...rest } = all; return rest; });
      await load();
    } catch (x) {
      setErr(`couldn't lift it: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }

  return (
    <div class="dx-reports dx-bans">
      <Show when={admin() === true} fallback={<p class="dx-note">{admin() === null ? "Checking whether you are an admin…" : "Only an admin reads bans."}</p>}>
        <div class="dx-reports-states" role="tablist" aria-label="which bans">
          <For each={[{ id: "active" as const, label: "in force" }, { id: "all" as const, label: "every ban" }]}>
            {(s) => (
              <button type="button" role="tab" class="dx-forget" aria-selected={state() === s.id} classList={{ on: state() === s.id }} onClick={() => { setBans(null); setState(s.id); }}>
                {s.label}
              </button>
            )}
          </For>
          <button type="button" class="dx-forget" disabled={busy()} onClick={() => void load()}>refresh</button>
        </div>
        <Show when={bans()?.items.length} fallback={<p class="dx-note">{bans() ? (state() === "active" ? "Nobody is banned." : "No bans yet.") : "Loading…"}</p>}>
          <ul class="dx-reports-list">
            <For each={bans()!.items}>
              {(b) => (
                <li classList={{ "dx-report--closed": b.state !== "active" }}>
                  <p class="dx-report-head">
                    <b>{b.target.gamertag}</b> {untilText(b.until)}
                    <span> by {b.by ?? "the operator"}, {b.at ? ago(b.at) : "at an unknown time"}{b.report ? `, for report ${b.report}` : ""}</span>
                  </p>
                  <p class="dx-report-reason">{b.reason}</p>
                  <Show
                    when={b.state === "active"}
                    fallback={
                      <p class="dx-note">
                        {STATE_WORDS[b.state]}
                        {b.lifted ? ` by ${b.lifted.by ?? "the operator"} ${ago(b.lifted.at)}${b.lifted.note ? `: ${b.lifted.note}` : ""}` : ""}
                      </p>
                    }
                  >
                    <form class="dx-report-form" onSubmit={(e) => { e.preventDefault(); void lift(b); }}>
                      <input
                        aria-label={`why lift ${b.target.gamertag}'s ban`} placeholder="note (optional)" maxLength={500} value={notes()[b.id] ?? ""}
                        onInput={(e) => { const v = e.currentTarget.value; setNotes((all) => ({ ...all, [b.id]: v })); }}
                      />
                      <button type="submit" class="dx-forget" disabled={busy()}>lift</button>
                    </form>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
      </Show>
    </div>
  );
}

/** Ban `target`, optionally answering a report; `onDone` after it lands. */
export function BanForm(props: { target: { xuid: string; gamertag: string }; report?: number; onDone?: () => void }) {
  const [open, setOpen] = createSignal(false);
  const [reason, setReason] = createSignal("");
  const [days, setDays] = createSignal<number | null>(BAN_LENGTHS[0]!.days);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);

  async function submit() {
    const c = ctx();
    if (!c || busy()) return;
    const problem = banProblem(reason());
    if (problem) { setErr(problem); return; }
    setBusy(true); setErr(null);
    try {
      await banAccount(c.api, c.aud, c.me, props.target.xuid, { reason: reason(), days: days(), report: props.report ?? null });
      setOpen(false); setReason("");
      props.onDone?.();
    } catch (x) {
      setErr(`couldn't ban: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }

  return (
    <Show when={open()} fallback={<button type="button" class="dx-forget" onClick={() => setOpen(true)}>ban</button>}>
      <form class="dx-report-form dx-ban-form" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <input
          aria-label={`why ban ${props.target.gamertag}`} placeholder="reason, shown to the player" maxLength={500} value={reason()}
          onInput={(e) => setReason(e.currentTarget.value)}
        />
        <select aria-label="how long" value={String(days())} onChange={(e) => setDays(e.currentTarget.value === "null" ? null : Number(e.currentTarget.value))}>
          <For each={BAN_LENGTHS}>{(l) => <option value={String(l.days)}>{l.label}</option>}</For>
        </select>
        <button type="submit" class="dx-forget" disabled={busy() || !reason().trim()}>ban</button>
        <button type="button" class="dx-forget" disabled={busy()} onClick={() => { setOpen(false); setErr(null); }}>cancel</button>
      </form>
      <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
    </Show>
  );
}

/** A player's ban as an admin sees it on their screen: in force with a
 *  lift, or a way to ban them. */
export function BanControl(props: { target: { xuid: string; gamertag: string } }) {
  const [ban, setBan] = createSignal<Notice | null | undefined>(undefined);
  const [err, setErr] = createSignal<string | null>(null);
  async function load() {
    const c = ctx();
    if (!c) return;
    try { setBan(await fetchBan(c.api, c.aud, c.me, props.target.xuid)); setErr(null); } catch (x) { setErr((x as Error).message); }
  }
  createEffect(on(() => [props.target.xuid, live.server().base, identity.me()?.xuid], () => void load()));
  async function lift() {
    const c = ctx();
    if (!c) return;
    try { await liftBan(c.api, c.aud, c.me, props.target.xuid, ""); await load(); } catch (x) { setErr(`couldn't lift it: ${(x as Error).message}`); }
  }
  return (
    <div class="dx-ban-control">
      <Show when={ban() !== undefined}>
        <Show when={ban()} fallback={<BanForm target={props.target} onDone={() => void load()} />}>
          {(b) => (
            <p class="dx-note">
              banned {untilText(b().until)}: {b().reason}{" "}
              <button type="button" class="dx-forget" onClick={() => void lift()}>lift</button>
            </p>
          )}
        </Show>
      </Show>
      <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
    </div>
  );
}

/** The signed-in player's own ban, if they have one. */
export function BanNotice() {
  const [ban, setBan] = createSignal<Notice | null>(null);
  createEffect(on(() => [identity.me()?.xuid, live.server().base, live.wellKnown()?.id], () => {
    const c = ctx();
    setBan(null);
    if (!c) return;
    fetchBan(c.api, c.aud, c.me, c.me.xuid).then(setBan, () => setBan(null));
  }));
  return (
    <Show when={ban()}>
      {(b) => (
        <p class="dx-error dx-ban-notice" role="status">
          This account is banned {untilText(b().until)}: {b().reason}. Until then the game can't sign in to h2 live.
        </p>
      )}
    </Show>
  );
}
