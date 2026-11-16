// The dashboard's server screens (/dash/server, docs/live/live-admin.md §5):
//   - the overview: the server's public facts for everyone; for an admin,
//     "your server": uptime, the tunnel, game art, admins, the log;
//   - /dash/server/invites: an admin makes, copies and revokes invite codes;
//   - /dash/server/connect: the switcher to another server.
// Dashboard.tsx owns the screen layout (the big pod, the four small pods
// and the prompts); these are the info plates and their data.

import { For, Match, Show, Switch, createEffect, createMemo, createSignal, on, onCleanup, type Accessor } from "solid-js";
import type { Action } from "./input";
import { A } from "@solidjs/router";
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
  fetchServerFacts,
  formatUptime,
  reachWords,
  revokeInvite,
  type AdminStatus,
  type InviteItem,
  type ServerFacts,
} from "../admin/server";
import { Plate } from "./parts";

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
  const admins = () => s().admins.map((a) => (a.source === "granted" ? `${a.gamertag} (granted)` : a.gamertag)).join(", ") || "none";
  return (
    <Plate small="your server" title={s().id || host()} main>
      <dl class="dx-server-facts dx-server-facts--admin">
        <dt>up</dt><dd>{formatUptime(s().uptime_s)} <span>{s().software ?? ""}</span></dd>
        <dt>public</dt><dd title={s().public.url ?? ""}>{s().public.url ? `${s().public.url} ` : ""}<span>{reachWords(s().public)}</span></dd>
        <dt>sign-up</dt><dd>{yes(s().invite_only, "invite only", "open")} <span>{s().invites.open} open invites</span></dd>
        <dt>reads</dt><dd>{yes(s().private, "private", "public")}</dd>
        <dt>game art</dt><dd>{artWords(s().game_art)}</dd>
        <dt>admins</dt><dd title={admins()}>{admins()}</dd>
        <dt>federation</dt><dd>{yes(s().federation, "on", "off")}</dd>
        <dt>event log</dt><dd>{s().log.head} <span>events</span> {s().log.store} <span>store</span></dd>
        <Show when={s().counts}>
          {(c) => <><dt>players</dt><dd>{c().accounts} <span>accounts</span> {c().online} <span>online</span> {c().live_games} <span>live</span></dd></>}
        </Show>
        <Show when={s().data_dir}><dt>data</dt><dd title={s().data_dir!}>{s().data_dir}</dd></Show>
        <dt>content</dt><dd><A class="dx-plain-link" href="/stats/admin?tab=content">custom maps, reports, uploads</A></dd>
      </dl>
    </Plate>
  );
}

/** A line of text with a copy button. */
export function CopyCode(props: { text: string; label?: string }) {
  const [done, setDone] = createSignal(false);
  return (
    <span class="dx-copy">
      <code>{props.text}</code>
      <button type="button" class="dx-forget" onClick={() => void copyText(props.text).then((ok) => { setDone(ok); setTimeout(() => setDone(false), 1500); })}>
        {done() ? "copied" : props.label ?? "copy"}
      </button>
    </span>
  );
}

export async function copyText(text: string): Promise<boolean> {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}

export interface InviteControls {
  create(): void;
  copy(): void;
  /** One step of keyboard or gamepad input on the invite rows; whether it meant anything. */
  act(a: Action): boolean;
  /** The selected row is an open invite (X revokes it). */
  revokable: Accessor<boolean>;
}

type InviteRow =
  | { kind: "days" }
  | { kind: "code" }
  | { kind: "create" }
  | { kind: "made"; invite: string }
  | { kind: "open"; item: InviteItem };

/** Making, copying and revoking invite codes (admins): a column of rows
 *  (blades) moved through with up and down; A acts on the row, X revokes. */
export function InvitesPlate(props: { info: ServerInfo; bind: (c: InviteControls) => void }) {
  const [days, setDays] = createSignal<number>(7);
  const [custom, setCustom] = createSignal("");
  const [made, setMade] = createSignal<{ invite: string; exp: number; custom?: boolean } | null>(null);
  const [list, setList] = createSignal<InviteItem[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const [flash, setFlash] = createSignal<string | null>(null);
  const [sel, setSel] = createSignal(0);
  const ctx = () => {
    const me = identity.me(), wk = live.wellKnown();
    return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
  };
  const say = (t: string) => { setFlash(t); setTimeout(() => setFlash((f) => (f === t ? null : f)), 1800); };
  async function load() {
    const c = ctx();
    if (!c || props.info.admin() !== true) return;
    try { setList(await fetchInvites(c.api, c.aud, c.me)); } catch (x) { setErr((x as Error).message); }
  }
  createEffect(on(() => [props.info.admin(), live.server().base], () => void load()));
  async function create() {
    const c = ctx();
    if (!c || busy()) return;
    const want = custom().trim();
    const bad = want ? customCodeError(want) : null;
    if (bad) { setErr(`own code: ${bad}`); return; }
    setBusy(true); setErr(null);
    try {
      const r = await createInvite(c.api, c.aud, c.me, days(), undefined, undefined, want || undefined);
      setMade(r);
      setCustom("");
      if (await copyText(r.invite)) say("code copied");
      await load();
      props.info.refresh();
    } catch (x) {
      setErr(`couldn't make an invite: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }
  async function copy(code?: string | null) {
    const c = code ?? made()?.invite ?? list().find((i) => i.invite)?.invite;
    if (c && (await copyText(c))) say("code copied");
  }
  async function revoke(jti: string) {
    const c = ctx();
    if (!c) return;
    setErr(null);
    try {
      await revokeInvite(c.api, c.aud, c.me, jti);
      if (made() && list().find((i) => i.jti === jti)?.invite === made()!.invite) setMade(null);
      await load();
      props.info.refresh();
      say("invite revoked");
    } catch (x) { setErr(`couldn't revoke: ${(x as Error).message}`); }
  }
  const open = () => list().filter((i) => i.state === "open");
  const recent = () => list().filter((i) => i.state !== "open").slice(0, 3);
  const rows = createMemo((): InviteRow[] => {
    const m = made();
    return [
      { kind: "days" }, { kind: "code" }, { kind: "create" },
      ...(m ? [{ kind: "made" as const, invite: m.invite }] : []),
      ...open().filter((i) => !m || i.invite !== m.invite).map((item) => ({ kind: "open" as const, item })),
    ];
  });
  createEffect(() => { if (sel() >= rows().length) setSel(Math.max(0, rows().length - 1)); });
  const focusRow = (i: number) => queueMicrotask(() => document.getElementById(`dash-inv-row-${i}`)?.focus({ preventScroll: false }));
  const move = (i: number) => { setSel(i); focusRow(i); };
  function activate(r: InviteRow | undefined) {
    if (!r) return;
    switch (r.kind) {
      case "days": setDays(nextInviteDays(days())); break;
      case "code": focusRow(sel()); break;
      case "create": void create(); break;
      case "made": void copy(r.invite); break;
      case "open": if (r.item.invite) void copy(r.item.invite); else say("custom code: you chose it, so it isn't shown"); break;
    }
  }
  const selected = () => rows()[sel()];
  const revokable = () => selected()?.kind === "open";
  props.bind({
    create: () => void create(),
    copy: () => void copy(),
    revokable,
    act(a) {
      if (props.info.admin() !== true) return false;
      const n = rows().length;
      switch (a) {
        case "up": move(Math.max(0, sel() - 1)); return true;
        case "down": move(Math.min(n - 1, sel() + 1)); return true;
        case "left": case "right":
          if (selected()?.kind !== "days") return false;
          setDays(nextInviteDays(days(), a === "right" ? 1 : -1));
          return true;
        case "select": activate(selected()); return true;
        case "alt": { const r = selected(); if (r?.kind === "open") void revoke(r.item.jti); return r?.kind === "open"; }
        default: return false;
      }
    },
  });
  const until = (exp: number) => new Date(exp * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const daysWord = (d: number) => (d === 1 ? "1 day" : `${d} days`);
  return (
    <Plate small="h2 live" title="invites" main>
      <Show when={props.info.admin() === true} fallback={<p class="dx-note">Only an admin of {host()} makes invite codes.</p>}>
        <dl>
          <dt>sign-up</dt><dd>{props.info.facts()?.invite_only ? "invite code needed" : "open to anyone"}</dd>
        </dl>
        <ul class="dx-blades" role="listbox" aria-label="invites">
          <For each={rows()}>
            {(r, i) => {
              const on = () => sel() === i();
              const common = { id: `dash-inv-row-${i()}`, onMouseEnter: () => setSel(i()), onFocus: () => setSel(i()) };
              return (
                <li classList={{ on: on() }} role="option" aria-selected={on()}>
                  <Switch>
                    <Match when={r.kind === "days"}>
                      <button type="button" class="dx-blade" {...common} onClick={() => activate(r)}>
                        <span>lasts</span><b>‹ {daysWord(days())} ›</b>
                      </button>
                    </Match>
                    <Match when={r.kind === "code"}>
                      <label class="dx-blade dx-blade--input" for={`dash-inv-row-${i()}`}>
                        <span>own code</span>
                        <input
                          {...common} data-nav="rows" value={custom()} maxLength={CUSTOM_CODE_MAX} autocomplete="off" spellcheck={false}
                          placeholder="optional" onInput={(e) => setCustom(e.currentTarget.value)}
                          onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void create(); } }}
                        />
                      </label>
                    </Match>
                    <Match when={r.kind === "create"}>
                      <button type="button" class="dx-blade dx-blade--go" {...common} disabled={busy()} onClick={() => activate(r)}>
                        <span>{busy() ? "making invite" : "new invite"}</span><b>{custom().trim() ? "own code" : "random code"}</b>
                      </button>
                    </Match>
                    <Match when={r.kind === "made" ? r : null}>
                      {(m) => (
                        <button type="button" class="dx-blade dx-blade--code" {...common} onClick={() => activate(r)}>
                          <span>new</span><b class="dx-invite-code">{m().invite}</b>
                        </button>
                      )}
                    </Match>
                    <Match when={r.kind === "open" ? r.item : null}>
                      {(it) => (
                        <button type="button" class="dx-blade" {...common} onClick={() => activate(r)}>
                          <span>{it().custom ? "custom code" : <code class="dx-invite-code">{it().invite}</code>}</span>
                          <b>until {until(it().exp)}</b>
                        </button>
                      )}
                    </Match>
                  </Switch>
                </li>
              );
            }}
          </For>
          <For each={recent()}>
            {(i) => <li class="dx-blade-old"><span>{i.custom ? "custom code" : "invite"}</span><b>{i.state}{i.used_by_gamertag ? ` by ${i.used_by_gamertag}` : ""}</b></li>}
          </For>
        </ul>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
        <Show when={flash()}><p class="dx-note dx-flash" role="status">{flash()}</p></Show>
        <Show when={!props.info.facts()?.invite_only}>
          <p class="dx-note">Sign-up is open here, so nobody needs a code yet.</p>
        </Show>
      </Show>
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
