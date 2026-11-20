// The dashboard's server screens (/dash/server, docs/live/live-admin.md §5).
// Everything about running the service is here, on the dashboard; the stats
// site's admin area keeps only Halo's own things (message of the day,
// playlists, custom content), reached from the "halo admin" pod.
//   - the overview: the server's public facts for everyone; for an admin,
//     "your server": uptime, the address, sign-up, players, game art, the log;
//   - /dash/server/invites: admins make, copy and revoke invite codes;
//   - /dash/server/my-invites: a granted player manages their own open invites;
//   - /dash/server/accounts: an admin's view of the accounts and the admins;
//   - /dash/server/reports: an admin's queue of player reports (Reports.tsx);
//   - /dash/server/bans: an admin's list of bans, with lift (Bans.tsx);
//   - /dash/server/import: anyone signs this browser in as an account the
//     CLI exported;
//   - /dash/server/connect: the switcher to another server.
// Dashboard.tsx owns the screen layout (the big pod, the four small pods
// and the prompts); these are the plates and their data. Actions on a plate
// are blades (Blades.tsx), driven by the same input as the rest.

import { Show, createEffect, createMemo, createSignal, on, onCleanup, type Accessor } from "solid-js";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { adminState } from "../admin/state";
import { forget, recentServers } from "../api/server";
import {
  CUSTOM_CODE_MAX,
  nextInviteDays,
  artWords,
  customCodeError,
  createInvite,
  fetchAdminStatus,
  fetchInvites,
  fetchMyInvites,
  fetchServerFacts,
  formatUptime,
  reachWords,
  revokeInvite,
  adminSourceWords,
  type AdminStatus,
  type InviteItem,
  type ServerFacts,
} from "../admin/server";
import { importAccounts } from "../admin/accountImport";
import { DASH_PLAYERS } from "../routes";
import { Plate } from "./parts";
import { Blades, type Blade, type BladeControls } from "./Blades";

export interface ServerInfo {
  facts: Accessor<ServerFacts | null>;
  status: Accessor<AdminStatus | null>;
  error: Accessor<string | null>;
  admin: Accessor<boolean | null>;
  refresh(): void;
}

/** The server's facts, and for an admin its status; re-read when the
 *  server or the account changes and every 15 s while shown. */
export function createServerInfo(active: Accessor<boolean>): ServerInfo {
  const [facts, setFacts] = createSignal<ServerFacts | null>(null);
  const [status, setStatus] = createSignal<AdminStatus | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const admin = adminState.isMeAdmin;
  let seq = 0;
  async function refresh() {
    const n = ++seq;
    const api = live.server().api;
    try {
      const f = await fetchServerFacts(api);
      if (n === seq) setFacts(f);
    } catch (x) {
      if (n === seq) setError((x as Error).message);
    }
    const me = identity.me(), wk = live.wellKnown();
    if (admin() !== true || !me || !wk) { if (n === seq) setStatus(null); return; }
    try {
      const s = await fetchAdminStatus(api, wk.id, me);
      if (n === seq) { setStatus(s); setError(null); }
    } catch (x) {
      if (n === seq) { setStatus(null); setError(`status: ${(x as Error).message}`); }
    }
  }
  createEffect(on(() => [active(), live.server().base, identity.me()?.xuid, admin(), live.wellKnown()?.id], () => {
    if (!active()) return;
    void refresh();
    const t = setInterval(() => void refresh(), 15_000);
    onCleanup(() => clearInterval(t));
  }));
  return { facts, status, error, admin, refresh: () => void refresh() };
}

const yes = (b: boolean, y: string, n: string) => (b ? y : n);
const host = () => live.wellKnown()?.id ?? new URL(live.server().base).host;
const linkWord = () => (live.status() === "live" ? "connected" : live.status());

/** The overview: public facts, or the admin's status. */
export function ServerPlate(props: { info: ServerInfo }) {
  return (
    <Show when={props.info.status()} fallback={<PublicPlate info={props.info} />}>
      {(s) => <AdminPlate s={s()} />}
    </Show>
  );
}

function PublicPlate(props: { info: ServerInfo }) {
  const f = () => props.info.facts();
  return (
    <Plate small={linkWord()} title={host()} main>
      <dl class="dx-server-facts">
        <dt>address</dt><dd>{f()?.url ?? live.server().base}</dd>
        <dt>software</dt><dd>{f()?.software ?? live.wellKnown()?.software ?? "unknown"}</dd>
        <Show when={f()}>
          {(f) => (
            <>
              <dt>sign-up</dt><dd>{yes(f().invite_only, "invite code needed", "open to anyone")}</dd>
              <dt>reads</dt><dd>{yes(f().private, "private (signed in only)", "public")}</dd>
              <Show when={f().counts}>
                {(c) => (
                  <>
                    <dt>accounts</dt><dd>{c().accounts}</dd>
                    <dt>now</dt><dd>{c().online} <span>online</span> {c().live_games} <span>live games</span></dd>
                  </>
                )}
              </Show>
            </>
          )}
        </Show>
        <Show when={props.info.admin() === true && !props.info.status()}>
          <dt>admin</dt><dd>{props.info.error() ?? "loading your server"}</dd>
        </Show>
      </dl>
    </Plate>
  );
}

function AdminPlate(props: { s: AdminStatus }) {
  const s = () => props.s;
  return (
    <Plate small="your server" title={s().id || host()} main>
      <dl class="dx-server-facts">
        <dt>up</dt><dd>{formatUptime(s().uptime_s)}</dd>
        <dt>reached</dt><dd title={s().public.url ?? ""}>{reachWords(s().public)}</dd>
        <dt>sign-up</dt><dd>{yes(s().invite_only, "invite only", "open to anyone")} <span>{s().invites.open} open invites</span></dd>
        <dt>reads</dt><dd>{yes(s().private, "private", "public")}</dd>
        <Show when={s().counts}>
          {(c) => <><dt>players</dt><dd>{c().accounts} <span>accounts</span> {c().online} <span>online</span> {c().live_games} <span>live</span></dd></>}
        </Show>
        <dt>game art</dt><dd>{artWords(s().game_art)}</dd>
        <dt>event log</dt><dd>{s().log.head} <span>events</span></dd>
      </dl>
    </Plate>
  );
}

export async function copyText(text: string): Promise<boolean> {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}

export interface InviteControls extends BladeControls {
  create(): void;
  copy(): void;
}

/** Making, copying and revoking invite codes. A acts on a row, X revokes. */
export function InvitesPlate(props: { info: ServerInfo; own?: boolean; bind: (c: InviteControls) => void }) {
  const [days, setDays] = createSignal<number>(7);
  const [custom, setCustom] = createSignal("");
  const [made, setMade] = createSignal<{ invite: string; jti: string; exp: number; custom?: boolean } | null>(null);
  const [list, setList] = createSignal<InviteItem[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const [flash, setFlash] = createSignal<string | null>(null);
  const ctx = () => {
    const me = identity.me(), wk = live.wellKnown();
    return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
  };
  const canUse = () => props.own ? adminState.myInviteGrant() !== null : props.info.admin() === true;
  const say = (t: string) => { setFlash(t); setTimeout(() => setFlash((f) => (f === t ? null : f)), 1800); };
  let scope = 0;
  let loadSeq = 0;
  async function load() {
    const c = ctx();
    if (!c || !canUse()) return;
    const n = ++loadSeq;
    try {
      const read = props.own ? fetchMyInvites : fetchInvites;
      const items = await read(c.api, c.aud, c.me);
      if (n !== loadSeq) return;
      setList(items);
      if (props.own) setMade((m) => m && items.some((i) => i.jti === m.jti) ? m : null);
    } catch (x) { if (n === loadSeq) setErr((x as Error).message); }
  }
  createEffect(on([canUse, () => live.server().api, () => live.wellKnown()?.id, () => identity.me()?.kid], () => {
    setList([]); setMade(null); setErr(null); setFlash(null); setCustom(""); setBusy(false);
    onCleanup(() => { scope++; loadSeq++; });
    if (!canUse()) return;
    void load();
    if (props.own) {
      const timer = setInterval(() => void load(), 15_000);
      onCleanup(() => clearInterval(timer));
    }
  }));
  async function create() {
    const c = ctx();
    if (!c || !canUse() || busy()) return;
    const n = scope;
    const want = custom().trim();
    const bad = want ? customCodeError(want) : null;
    if (bad) { setErr(`own code: ${bad}`); return; }
    setBusy(true); setErr(null);
    try {
      const r = await createInvite(c.api, c.aud, c.me, days(), undefined, undefined, want || undefined);
      if (n !== scope) return;
      setMade(r);
      setCustom("");
      if (await copyText(r.invite) && n === scope) say("code copied");
      if (n !== scope) return;
      await load();
      if (n === scope && !props.own) props.info.refresh();
    } catch (x) {
      if (n === scope) setErr(`couldn't make an invite: ${(x as Error).message}`);
    } finally { if (n === scope) setBusy(false); }
  }
  async function copy(code?: string | null) {
    if (!canUse()) return;
    const c = code ?? made()?.invite ?? list().find((i) => i.invite)?.invite;
    if (c && (await copyText(c))) say("code copied");
  }
  async function revoke(jti: string) {
    const c = ctx();
    if (!c || !canUse()) return;
    const n = scope;
    setErr(null);
    try {
      await revokeInvite(c.api, c.aud, c.me, jti);
      if (n !== scope) return;
      if (made()?.jti === jti) setMade(null);
      await load();
      if (n !== scope) return;
      if (!props.own) props.info.refresh();
      say("invite revoked");
    } catch (x) { if (n === scope) setErr(`couldn't revoke: ${(x as Error).message}`); }
  }
  const until = (exp: number) => new Date(exp * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const daysWord = (d: number) => (d === 1 ? "1 day" : `${d} days`);
  const blades = createMemo((): Blade[] => {
    const m = made();
    const open = list().filter((i) => i.state === "open" && (!m || (i.jti !== m.jti && i.invite !== m.invite)));
    const recent = list().filter((i) => i.state !== "open").slice(0, 3);
    const revoker = (jti: string) => ({ label: "revoke", run: () => void revoke(jti) });
    return [
      { key: "days", label: "lasts", value: `‹ ${daysWord(days())} ›`, act: () => setDays(nextInviteDays(days())), step: (by) => setDays(nextInviteDays(days(), by)) },
      { key: "code", label: "own code", field: { value: custom(), onInput: setCustom, placeholder: "optional", maxLength: CUSTOM_CODE_MAX, onEnter: () => void create() } },
      { key: "create", label: busy() ? "making invite" : "new invite", value: custom().trim() ? "own code" : "random code", act: () => void create(), disabled: busy(), go: true },
      ...(m ? [{ key: `made-${m.invite}`, label: "new", value: <span class="dx-invite-code">{m.invite}</span>, act: () => void copy(m.invite), alt: list().some((i) => i.jti === m.jti && i.state === "open") ? revoker(m.jti) : undefined } satisfies Blade] : []),
      ...open.map((it): Blade => ({
        key: it.jti,
        label: it.custom ? "custom code" : <span class="dx-invite-code">{it.invite}</span>,
        value: `until ${until(it.exp)}`,
        act: () => (it.invite ? void copy(it.invite) : say("custom code: you chose it, so it isn't shown")),
        alt: revoker(it.jti),
      })),
      ...recent.map((it): Blade => ({ key: it.jti, label: it.custom ? "custom code" : "invite", value: `${it.state}${it.used_by_gamertag ? ` by ${it.used_by_gamertag}` : ""}`, old: true })),
    ];
  });
  let ctl: BladeControls | null = null;
  props.bind({
    create: () => void create(),
    copy: () => void copy(),
    sel: () => ctl?.sel() ?? 0,
    alt: () => ctl?.alt() ?? null,
    act: (a) => (canUse() && ctl ? ctl.act(a) : false),
  });
  return (
    <Plate small="h2 live" title={props.own && canUse() ? "invite someone" : "invites"} main>
      <Show when={canUse()} fallback={<p class="dx-note">An admin of {host()} must enable invite access for this account.</p>}>
        <dl>
          <dt>sign-up</dt><dd>{props.info.facts()?.invite_only ? "invite code needed" : "open to anyone"}</dd>
        </dl>
        <Blades id="dash-inv-row" label="invites" blades={blades()} bind={(c) => { ctl = c; }} />
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
        <Show when={flash()}><p class="dx-note dx-flash" role="status">{flash()}</p></Show>
        <Show when={!props.info.facts()?.invite_only}>
          <p class="dx-note">Sign-up is open here, so nobody needs a code yet.</p>
        </Show>
      </Show>
    </Plate>
  );
}

/** The accounts and who the admins are (admins). Granting stays with
 *  whoever runs the server, so this lists and leads on; it changes nothing. */
export function AccountsPlate(props: { info: ServerInfo; go: (href: string) => void; bind: (c: BladeControls) => void }) {
  const blades = createMemo((): Blade[] => {
    const s = props.info.status();
    return [
      { key: "players", label: "every player", value: "players", act: () => props.go(DASH_PLAYERS) },
      { key: "reports", label: "player reports", value: "review", act: () => props.go("/dash/server/reports") },
      { key: "bans", label: "bans", value: "lift", act: () => props.go("/dash/server/bans") },
      { key: "import", label: "import account", value: "account file", act: () => props.go("/dash/server/import") },
      ...(s?.admins ?? []).map((a): Blade => ({ key: `admin-${a.gamertag}`, label: <>admin <b class="dx-blade-name">{a.gamertag}</b></>, value: adminSourceWords(a.source) })),
    ];
  });
  return (
    <Plate small="your server" title="accounts" main>
      <Show when={props.info.admin() === true} fallback={<p class="dx-note">Only an admin of {host()} sees its accounts.</p>}>
        <dl>
          <Show when={props.info.status()?.counts ?? props.info.facts()?.counts}>
            {(c) => <><dt>accounts</dt><dd>{c().accounts} <span>signed up</span> {c().online} <span>online</span></dd></>}
          </Show>
          <dt>sign-up</dt><dd>{props.info.facts()?.invite_only ? "invite code needed" : "open to anyone"}</dd>
          <dt>admins</dt><dd>{props.info.status()?.admins.length ?? "…"}</dd>
        </dl>
        <Blades id="dash-acct-row" label="accounts" blades={blades()} bind={props.bind} />
        <p class="dx-note">Admins are chosen by whoever runs the server, on its own machine.</p>
      </Show>
    </Plate>
  );
}

/** Signing this browser in as an account the game or the CLI exported (its
 *  account file). Open to anyone: it is a way in, like recovering. */
export function ImportPlate(props: { onDone: () => void; bind: (c: BladeControls) => void }) {
  const [text, setText] = createSignal("");
  const [err, setErr] = createSignal<string | null>(null);
  const [ok, setOk] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  let file!: HTMLInputElement;
  async function run() {
    if (busy() || !text().trim()) return;
    setBusy(true); setErr(null); setOk(null);
    const wk = live.wellKnown();
    const r = await importAccounts(text(), {
      api: live.server().api, base: live.server().base, id: wk?.id ?? "",
      add: (a) => identity.add(a),
    });
    setBusy(false);
    if (!r.ok) { setErr(r.error); return; }
    setText("");
    adminState.reset();
    setOk(`signed in as ${r.gamertags.join(", ")}`);
    props.onDone();
  }
  async function pick(f: File | undefined) {
    if (f) setText(await f.text());
  }
  const blades = createMemo((): Blade[] => [
    { key: "file", label: "choose the account file", value: "browse", act: () => file.click() },
    { key: "go", label: busy() ? "checking" : "import and sign in", act: () => void run(), disabled: busy() || !text().trim(), go: true },
  ]);
  return (
    <Plate small="h2 live" title="import account" main>
      <div class="dx-import">
        <textarea
          id="dash-import-text" class="dx-paste" rows={4} spellcheck={false} value={text()}
          placeholder="paste the account file here, or choose it below"
          aria-label="account file" onInput={(e) => setText(e.currentTarget.value)}
        />
        <input ref={file} type="file" accept=".json,application/json" hidden onChange={(e) => void pick(e.currentTarget.files?.[0])} />
        <Blades id="dash-imp-row" label="import" blades={blades()} bind={props.bind} />
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
        <Show when={ok()}><p class="dx-note dx-flash" role="status">{ok()}</p></Show>
        <p class="dx-note">For an account made in the game. The key is checked with {host()}, then kept in this browser, like a new account's.</p>
      </div>
    </Plate>
  );
}

/** The switcher: connect to another server, forget the others. */
export function ConnectPlate(props: { onUse: (input: string) => Promise<void>; busy: boolean; err: string | null }) {
  const [input, setInput] = createSignal("");
  const [tick, setTick] = createSignal(0);
  const others = () => { tick(); return recentServers().filter((r) => r.base !== live.server().base); };
  return (
    <Plate small={linkWord()} title="another server" main>
      <form id="dash-server-form" class="dx-form" onSubmit={(e) => { e.preventDefault(); if (input().trim()) void props.onUse(input()).then(() => setInput("")); }}>
        <dl><dt>now on</dt><dd>{live.server().base}</dd></dl>
        <label for="dash-server">address</label>
        <input id="dash-server" placeholder="live.example.net" value={input()} onInput={(e) => setInput(e.currentTarget.value)} autocomplete="off" spellcheck={false} />
        <button type="submit" class="dx-go" disabled={props.busy || !input().trim()}>{props.busy ? "connecting" : "connect"}</button>
        <Show when={props.err}><p class="dx-error" role="alert">{props.err}</p></Show>
        <Show when={others().length}>
          <button type="button" class="dx-forget" onClick={() => { for (const r of others()) forget(r.base); setTick(tick() + 1); }}>forget other servers</button>
        </Show>
      </form>
    </Plate>
  );
}
