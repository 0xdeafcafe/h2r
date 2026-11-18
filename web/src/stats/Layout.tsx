// The stats site shell and its shared chrome: a fixed 902px column on a navy
// page, the header with the site name and the Welcome box, the black nav
// strip with search, breadcrumbs, banner strips, section titles, pill menus,
// blue-headed boxes, tabs and pagers.

import { For, Show, createMemo, createSignal, type JSX } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { accountByGamertag, liveGames } from "../state/reducer";
import { social } from "../state/social";
import { friendPresence } from "./clanBits";
import Emblem from "../components/Emblem";
import { VerifyMark } from "../components/bits";
import Wordmark from "./Wordmark";
import { adminState } from "../admin/state";
import { DASHBOARD_EXIT, STATS_HOME, statsNav } from "./modes";
import { gameImage } from "../state/gameart";
import { bannerKey } from "../lib/gameart";
import { STATS_NAME } from "../brand";
import { STATS_ROOT, statsPath } from "../routes";

const playerHref = (tag: string) => `${STATS_ROOT}/players/${encodeURIComponent(tag)}`;

export default function StatsLayout(props: { children?: JSX.Element }) {
  const navigate = useNavigate();
  const [q, setQ] = createSignal("");
  const online = createMemo(() => Object.values(live.state.accounts).filter((a) => a.online).length);
  const friendsOnline = createMemo(() => {
    const me = identity.me();
    if (!me) return 0;
    return (social.mine()?.friends ?? []).filter((f) => friendPresence(f).kind !== "offline").length;
  });
  function search(e: Event) {
    e.preventDefault();
    const t = q().trim();
    if (!t) return;
    const a = accountByGamertag(live.state, t)
      ?? Object.values(live.state.accounts).find((x) => x.gamertag.toLowerCase().includes(t.toLowerCase()));
    navigate(a ? playerHref(a.gamertag) : `${STATS_ROOT}/players?q=${encodeURIComponent(t)}`);
  }
  return (
    <div class="stats">
      <div class="s-column">
        <header class="s-top">
          <A href={DASHBOARD_EXIT.href} end class="s-exit" title={DASHBOARD_EXIT.title}>{DASHBOARD_EXIT.label}</A>
          <A href={STATS_HOME} class="s-logo" aria-label={`${STATS_NAME} home`}><Wordmark height={40} /></A>
          <div class="s-welcome">
            <Show
              when={identity.me()}
              fallback={
                <>
                  <span class="s-welcome-pic"><Emblem seed="guest" size={56} /></span>
                  <ul>
                    <li class="s-welcome-hi">Welcome, <b>guest</b>!</li>
                    <li class="ico ico-bolt"><A href={statsPath("/players")}>Players Online ({online()})</A></li>
                    <li class="ico ico-mail"><A href={statsPath("/games")}>Games Live ({liveGames(live.state).length})</A></li>
                    <li><A href="/dash/signup">Sign Up</A><i>|</i><A href="/">Dashboard</A></li>
                  </ul>
                </>
              }
            >
              {(me) => (
                <>
                  <A href={playerHref(me().gamertag)} class="s-welcome-pic"><Emblem seed={me().xuid} size={56} /></A>
                  <ul>
                    <li class="s-welcome-hi">Welcome, <b>{me().gamertag}</b>!</li>
                    <li class="ico ico-bolt"><A href="/dash/friends">Friends Online ({friendsOnline()})</A></li>
                    <li class="ico ico-mail"><A href={statsPath("/games")}>Games Live ({liveGames(live.state).length})</A></li>
                    <li><A href={playerHref(me().gamertag)}>My Profile</A><i>|</i><A href={`${playerHref(me().gamertag)}/history`}>History</A><i>|</i><A href={`${playerHref(me().gamertag)}/career`}>Career</A></li>
                  </ul>
                  <button class="s-signout" onClick={() => identity.signOut()}>Sign Out</button>
                </>
              )}
            </Show>
          </div>
        </header>
        <nav class="s-nav" aria-label="Stats">
          <For each={statsNav(identity.me() ?? null, adminState.isMeAdmin() === true)}>{(n) => <A href={n.href} end={n.end}>{n.label}</A>}</For>
          <form class="s-search" onSubmit={search} role="search">
            <label class="sr-only" for="s-q">Find a gamertag</label>
            <input id="s-q" placeholder="Enter Gamertag" value={q()} onInput={(e) => setQ(e.currentTarget.value)} />
            <button type="submit" class="s-btn">search</button>
          </form>
        </nav>
        <main class="s-main">{props.children}</main>
        <footer class="s-foot">
          <p>h2 live is an independent community service for h2recomp. Not affiliated with any game publisher.</p>
          <ul>
            <li><A href="/">Dashboard</A></li>
            <li><A href={statsPath("/players")}>Players</A></li>
            <li><A href={statsPath("/ranks")}>Ranked</A></li>
            <li><A href={statsPath("/events")}>Event Log</A></li>
            <li><A href="/dash/server">{live.wellKnown()?.id ?? new URL(live.server().base).host}</A></li>
            <li>position {live.state.position}</li>
          </ul>
        </footer>
      </div>
    </div>
  );
}

export function Crumbs(props: { trail: Array<[string, string?]> }) {
  const online = createMemo(() => Object.values(live.state.accounts).filter((a) => a.online).length);
  const games = createMemo(() => liveGames(live.state).length);
  return (
    <div class="s-crumbs">
      <span class="s-crumbs-trail">
        <For each={props.trail}>
          {([label, href], i) => (
            <>
              {i() > 0 && <span class="s-crumb-sep"> : </span>}
              {href ? <A href={href}>{label}</A> : <span>{label}</span>}
            </>
          )}
        </For>
      </span>
      <span class="s-crumbs-right">
        {online()} h2 live Players Online ({games()} {games() === 1 ? "Game" : "Games"} Live)
        <Show when={live.status() !== "live"}><span class="s-conn"> · {live.status()}</span></Show>
      </span>
    </div>
  );
}

/** The banner under the breadcrumbs: shots of the server's own maps when
 *  its game art has them (`banner/home`, `banner/strip`), else the art
 *  pack's own picture. `kind` picks which; home's is the tall one. */
export function Strip(props: { kind: "home" | "players"; children?: JSX.Element }) {
  const shot = () => gameImage(bannerKey(props.kind));
  return (
    <div class={`s-strip s-strip--${props.kind}`} classList={{ "s-strip--shot": !!shot() }}>
      <div class="s-strip-art" aria-hidden="true" style={shot() ? { "background-image": `url("${shot()!.url}")` } : undefined} />
      <div class="s-strip-mark" aria-hidden="true"><Wordmark height={props.kind === "home" ? 34 : 24} /><small>{{ home: "online", players: "players" }[props.kind]}</small></div>
      {props.children}
    </div>
  );
}

/** Section title with the small "Return to ..." link beside it. */
export function Title(props: { children: JSX.Element; back?: [string, string]; right?: JSX.Element; help?: string; class?: string }) {
  return (
    <div class={`s-title ${props.class ?? ""}`}>
      <h1>{props.children}</h1>
      <Show when={props.back}>{(b) => <A href={b()[1]} class="s-title-back">{b()[0]}</A>}</Show>
      <Show when={props.right}><span class="s-title-right">{props.right}</span></Show>
      <Show when={props.help}><span class="s-help" title={props.help} aria-label={props.help}>?</span></Show>
    </div>
  );
}

/** A blue-headed box: white title (with an optional italic caption), dark body. */
export function Panel(props: { title: JSX.Element; sub?: JSX.Element; right?: JSX.Element; class?: string; children?: JSX.Element; id?: string }) {
  return (
    <section class={`s-box ${props.class ?? ""}`} id={props.id}>
      <header class="s-box-head">
        <div><h3>{props.title}</h3><Show when={props.sub}><p>{props.sub}</p></Show></div>
        <Show when={props.right}><span class="s-box-right">{props.right}</span></Show>
      </header>
      <div class="s-box-body">{props.children}</div>
    </section>
  );
}

/** A plain grey bordered block with a strip heading, as inside game reports. */
export function Block(props: { title?: JSX.Element; right?: JSX.Element; class?: string; children?: JSX.Element }) {
  return (
    <section class={`s-block ${props.class ?? ""}`}>
      <Show when={props.title}>
        <header class="s-block-head"><h3>{props.title}</h3><Show when={props.right}><span class="s-block-right">{props.right}</span></Show></header>
      </Show>
      {props.children}
    </section>
  );
}

export interface TabDef { id: string; label: JSX.Element; href?: string }

/** Big rounded tabs; either links (href) or buttons driving `onPick`. */
export function Tabs(props: { tabs: TabDef[]; active: string; onPick?: (id: string) => void; small?: boolean; label?: string }) {
  return (
    <div class="s-tabs" classList={{ "s-tabs--small": !!props.small }} role="tablist" aria-label={props.label}>
      <For each={props.tabs}>
        {(t) => (
          <Show
            when={t.href}
            fallback={<button role="tab" aria-selected={props.active === t.id} classList={{ on: props.active === t.id }} onClick={() => props.onPick?.(t.id)}>{t.label}</button>}
          >
            <A href={t.href!} role="tab" aria-selected={props.active === t.id} classList={{ on: props.active === t.id }} noScroll>{t.label}</A>
          </Show>
        )}
      </For>
    </div>
  );
}

export function Pager(props: { page: number; pages: number; total: number; per: number; onPage: (p: number) => void }) {
  const from = () => (props.total ? props.page * props.per + 1 : 0);
  const to = () => Math.min(props.total, (props.page + 1) * props.per);
  const shown = () => {
    const n = Math.max(1, props.pages);
    const start = Math.max(0, Math.min(props.page - 3, n - 7));
    return Array.from({ length: Math.min(7, n) }, (_, i) => start + i);
  };
  return (
    <div class="s-pager">
      Change page: <button disabled={props.page === 0} onClick={() => props.onPage(props.page - 1)} aria-label="Previous page">&lt;</button>
      <For each={shown()}>
        {(i) => <button classList={{ on: i === props.page }} onClick={() => props.onPage(i)} aria-current={i === props.page ? "page" : undefined}>{i + 1}</button>}
      </For>
      <button disabled={props.page >= props.pages - 1} onClick={() => props.onPage(props.page + 1)} aria-label="Next page">&gt;</button>
      <span class="s-pager-sep">|</span>
      <span>Displaying page {props.page + 1} of {Math.max(1, props.pages)}, items {from()} to {to()} of {props.total}.</span>
    </div>
  );
}

export function Derived(props: { children?: JSX.Element; note?: string }) {
  return <span class="s-derived" title={props.note ?? "Derived in your browser from this server's game events; not served by the API."}>{props.children ?? "derived"}<sup>*</sup></span>;
}

export { VerifyMark, playerHref };
