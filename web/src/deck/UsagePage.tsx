// /dash/admin/usage: the Live admin's usage screen (docs/live/web-boundary.md
// §5, docs/live/live-api.md "Usage"). Who uploads and stores the most, what
// is written and sent, per day, for an admin of this server. Two reads of
// the overview (the range, and twice it for the tiles' change) and one of
// the accounts, signed as the browser's admin account; a refetch keeps the
// last answer on screen, dimmed, rather than blanking it.

import { Show, createEffect, createMemo, createSignal, on, onCleanup, type JSX } from "solid-js";
import { A } from "@solidjs/router";
import { DeckPage, Panel, Presets } from "./Deck";
import { USAGE_SORTS, usageView } from "./usageView";
import UsageBoard from "./UsageBoard";
import { fetchUsage, fetchUsageAccounts, type AccountSort, type AccountsPage, type UsageOverview } from "../admin/usage";
import { adminState } from "../admin/state";
import { LIVE_ADMIN } from "../routes";
import { identity } from "../state/identity";
import { live } from "../state/live";

export const USAGE_RANGES = [
  { value: 7, label: "7 days" },
  { value: 30, label: "30 days" },
  { value: 90, label: "90 days" },
] as const;


interface Loaded { cur: UsageOverview; wide: UsageOverview; accounts: AccountsPage; range: number; sort: AccountSort }

export default function UsagePage() {
  const [range, setRange] = createSignal<number>(30);
  const [sort, setSort] = createSignal<AccountSort>("uploaded");
  const [data, setData] = createSignal<Loaded | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const admin = adminState.isMeAdmin;
  let seq = 0;
  let loadedScope = "";
  onCleanup(() => { seq++; });

  createEffect(on(() => [range(), sort(), admin(), identity.me()?.xuid, live.wellKnown()?.id, live.server().base] as const, async () => {
    const me = identity.me(), wk = live.wellKnown();
    const n = ++seq, r = range(), s = sort(), api = live.server().api;
    const scope = admin() === true && me && wk ? JSON.stringify([me.xuid, wk.id, api]) : "";
    if (scope !== loadedScope) {
      loadedScope = scope;
      setData(null);
      setError(null);
    }
    if (!scope || !me || !wk) {
      setBusy(false);
      return;
    }
    setBusy(true);
    try {
      const [cur, wide, accounts] = await Promise.all([
        fetchUsage(api, wk.id, me, r),
        fetchUsage(api, wk.id, me, Math.min(400, r * 2)),
        fetchUsageAccounts(api, wk.id, me, r, s, 25),
      ]);
      if (n === seq) { setData({ cur, wide, accounts, range: r, sort: s }); setError(null); }
    } catch (x) {
      if (n === seq) setError((x as Error).message);
    } finally {
      if (n === seq) setBusy(false);
    }
  }));

  const view = createMemo(() => {
    const d = data();
    return d ? usageView(d.cur, d.wide, d.cur.to, d.range) : null;
  });

  const gate = (title: string, body: JSX.Element) => <Panel title={title}><p class="dk-note">{body}</p></Panel>;
  return (
    <DeckPage
      trail={[["dashboard", "/"], ["server", "/dash/server"], ["live admin", LIVE_ADMIN], ["usage"]]}
      title="usage"
      back={LIVE_ADMIN}
      filters={<Show when={admin() === true}>
        <Presets label="range" options={USAGE_RANGES} value={range()} onPick={setRange} />
        <Presets label="top accounts by" options={USAGE_SORTS} value={sort()} onPick={setSort} />
        <Show when={busy()}><span class="dk-filters-note">reading…</span></Show>
      </Show>}
    >
      <Show when={identity.me()} fallback={gate("sign in first", <>Usage is for this server's admins. <A href="/dash/signin">Sign in</A> as one.</>)}>
        {(me) => (
          <Show when={admin() === true} fallback={gate(admin() === null ? "checking" : "not an admin",
            admin() === null ? <>Asking the server whether {me().gamertag} is an admin…</> : <>{me().gamertag} is not an admin on this server.</>)}>
            <Show when={error()}>{(e) => <p class="dk-span-12 dk-state dk-state--bad" role="alert">Could not read usage. {e()}</p>}</Show>
            <Show when={view()} fallback={<Show when={!error()}><p class="dk-span-12 dk-state" role="status">Reading usage…</p></Show>}>
              {(v) => (
                <div class="dk-span-12 dk-grid dk-subgrid" classList={{ "dk-stale": busy() }}>
                  <p class="dk-span-12 dk-note">Server usage from {data()!.cur.from} to {data()!.cur.to} (UTC). The current day is in progress.</p>
                  <UsageBoard view={v()} range={data()!.range} accounts={data()!.accounts.items}
                    showRequestAccounting
                    savesStorageMeasured={data()!.cur.saves_storage_measured === true}
                    accountSavesStorageMeasured={data()!.accounts.saves_storage_measured === true}
                    accountsNote={`${data()!.accounts.items.length} of ${data()!.accounts.total}, last ${data()!.range} days`} sort={data()!.sort} />
                </div>
              )}
            </Show>
          </Show>
        )}
      </Show>
    </DeckPage>
  );
}
