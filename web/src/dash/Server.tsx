// The dashboard's server screens (/dash/server, docs/live-admin.md §5):
//   - the overview: the server's public facts for everyone; for an admin,
//     "your server": uptime, the tunnel, game art, admins, the log;
//   - /dash/server/invites: an admin makes, copies and revokes invite codes;
//   - /dash/server/connect: the switcher to another server.
// Dashboard.tsx owns the screen layout (the big pod, the four small pods
// and the prompts); these are the info plates and their data.

import { For, Show, createEffect, createSignal, on, onCleanup, type Accessor } from "solid-js";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { adminState } from "../admin/state";
import { forget, recentServers } from "../api/server";
import {
  INVITE_DAY_CHOICES,
  artWords,
  createInvite,
  fetchAdminStatus,
  fetchInvites,
  fetchServerFacts,
  formatUptime,
  grantCommand,
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
        <dt>new admin</dt><dd><CopyCode text={grantCommand("GAMERTAG")} /></dd>
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

export interface InviteControls { create(): void; copy(): void }

/** Making, copying and revoking invite codes (admins). */
export function InvitesPlate(props: { info: ServerInfo; bind: (c: InviteControls) => void }) {
  const [days, setDays] = createSignal<number>(7);
  const [made, setMade] = createSignal<{ invite: string; exp: number } | null>(null);
  const [list, setList] = createSignal<InviteItem[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const [copied, setCopied] = createSignal(false);
  const ctx = () => {
    const me = identity.me(), wk = live.wellKnown();
    return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
  };
  async function load() {
    const c = ctx();
    if (!c || props.info.admin() !== true) return;
    try { setList(await fetchInvites(c.api, c.aud, c.me)); } catch (x) { setErr((x as Error).message); }
  }
  createEffect(on(() => [props.info.admin(), live.server().base], () => void load()));
  async function create() {
    const c = ctx();
    if (!c || busy()) return;
    setBusy(true); setErr(null);
    try {
      const r = await createInvite(c.api, c.aud, c.me, days());
      setMade(r);
      setCopied(await copyText(r.invite));
      await load();
      props.info.refresh();
    } catch (x) {
      setErr(`couldn't make an invite: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }
  async function copy() {
    const m = made() ?? (list().find((i) => i.invite) ? { invite: list().find((i) => i.invite)!.invite!, exp: 0 } : null);
    if (m) setCopied(await copyText(m.invite));
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
    } catch (x) { setErr(`couldn't revoke: ${(x as Error).message}`); }
  }
  props.bind({ create: () => void create(), copy: () => void copy() });
  const open = () => list().filter((i) => i.state === "open");
  const recent = () => list().filter((i) => i.state !== "open").slice(0, 3);
  const until = (exp: number) => new Date(exp * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  return (
    <Plate small={props.info.facts()?.invite_only ? "invite only" : "sign-up open"} title="invites" main>
      <Show when={props.info.admin() === true} fallback={<p class="dx-note">Only an admin of {host()} makes invite codes.</p>}>
        <form id="dash-invite-form" class="dx-form dx-invites" onSubmit={(e) => { e.preventDefault(); void create(); }}>
          <label for="dash-invite-days">lasts</label>
          <span class="dx-days" role="radiogroup" aria-label="how long the invite lasts">
            <For each={INVITE_DAY_CHOICES}>
              {(d) => (
                <button type="button" role="radio" aria-checked={days() === d} classList={{ on: days() === d }} onClick={() => setDays(d)}>
                  {d === 1 ? "1 day" : `${d} days`}
                </button>
              )}
            </For>
          </span>
          <button type="submit" class="dx-go" disabled={busy()}>{busy() ? "making" : "new invite"}</button>
          <Show when={made()}>
            {(m) => (
              <>
                <label for="dash-invite-code">code</label>
                <input id="dash-invite-code" readOnly value={m().invite} onFocus={(e) => e.currentTarget.select()} />
                <p class="dx-note">{copied() ? "Copied. " : ""}One sign-up, until {until(m().exp)}. Send it to one friend.</p>
              </>
            )}
          </Show>
          <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
          <Show when={!props.info.facts()?.invite_only}>
            <p class="dx-note">Sign-up is open on this server, so nobody needs a code until it runs with --invite-only.</p>
          </Show>
        </form>
        <Show when={open().length || recent().length}>
          <ul class="dx-invite-list">
            <For each={open()}>
              {(i) => (
                <li>
                  <span>open until {until(i.exp)}{i.by_gamertag ? ` · ${i.by_gamertag}` : ""}</span>
                  <button type="button" class="dx-forget" onClick={() => void copyText(i.invite ?? "")}>copy</button>
                  <button type="button" class="dx-forget" onClick={() => void revoke(i.jti)}>revoke</button>
                </li>
              )}
            </For>
            <For each={recent()}>
              {(i) => <li class="dx-invite-old"><span>{i.state}{i.used_by_gamertag ? ` by ${i.used_by_gamertag}` : ""}{i.by_gamertag ? ` · from ${i.by_gamertag}` : ""}</span></li>}
            </For>
          </ul>
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
