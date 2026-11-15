// The stats site shell: fixed centre column, top bar with the site name and
// a welcome box, a dark nav strip with search, then the page.

import { For, Show, createMemo, createSignal, type JSX } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { accountByGamertag, liveGames } from "../state/reducer";
import { playedWith } from "../state/derive";
import Emblem from "../components/Emblem";
import { VerifyMark } from "../components/bits";

export default function StatsLayout(props: { children?: JSX.Element }) {
  const navigate = useNavigate();
  const [q, setQ] = createSignal("");
  const online = createMemo(() => Object.values(live.state.accounts).filter((a) => a.online).length);
  const friendsOnline = createMemo(() => {
    const me = identity.me();
    if (!me) return 0;
    return playedWith(live.state, me.xuid).filter((f) => live.state.accounts[f.xuid]?.online).length;
  });
  function search(e: Event) {
    e.preventDefault();
    const t = q().trim();
    if (!t) return;
    const a = accountByGamertag(live.state, t)
      ?? Object.values(live.state.accounts).find((x) => x.gamertag.toLowerCase().includes(t.toLowerCase()));
    navigate(a ? `/stats/players/${encodeURIComponent(a.gamertag)}` : `/stats/players?q=${encodeURIComponent(t)}`);
  }
  return (
    <div class="stats">
      <div class="s-column">
        <header class="s-top">
          <A href="/stats" class="s-logo">h2 <span>live</span></A>
          <div class="s-welcome">
            <Show
              when={identity.me()}
              fallback={
                <>
                  <Emblem seed="guest" size={46} />
                  <div>
                    <b>Welcome, guest</b>
                    <div>▪ <A href="/dash/signup">Sign up for h2 live</A></div>
                    <div>▪ {online()} players online</div>
                  </div>
                </>
              }
            >
              {(me) => (
                <>
                  <Emblem seed={me().xuid} size={46} />
                  <div>
                    <b>Welcome, {me().gamertag}</b>
                    <div>▪ <A href="/dash/friends">Friends Online ({friendsOnline()})</A></div>
                    <div>▪ <A href={`/stats/players/${encodeURIComponent(me().gamertag)}`}>My Service Record</A></div>
                  </div>
                  <button class="s-signout" onClick={() => identity.signOut()}>Sign Out</button>
                </>
              )}
            </Show>
          </div>
        </header>
        <nav class="s-nav" aria-label="Stats">
          <A href="/" end>Dashboard</A>
          <A href="/stats" end>Games</A>
          <A href="/stats/players">Players</A>
          <Show when={identity.me()}>{(me) => <A href={`/stats/players/${encodeURIComponent(me().gamertag)}`} end>My Stats</A>}</Show>
          <A href="/stats/events">Event Log</A>
          <form class="s-search" onSubmit={search} role="search">
            <label class="sr-only" for="s-q">Find a gamertag</label>
            <input id="s-q" placeholder="Enter gamertag" value={q()} onInput={(e) => setQ(e.currentTarget.value)} />
            <button type="submit">search</button>
          </form>
        </nav>
        <main class="s-main">{props.children}</main>
        <footer class="s-foot">
          <span>h2 live · an independent community service for h2recomp · not affiliated with any game publisher</span>
          <span>{live.wellKnown()?.id ?? live.server().base} · position {live.state.position}</span>
        </footer>
      </div>
    </div>
  );
}

export function Crumbs(props: { trail: Array<[string, string?]> }) {
  const online = createMemo(() => Object.values(live.state.accounts).filter((a) => a.online).length);
  return (
    <div class="s-crumbs">
      <span>
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
        {liveGames(live.state).length} games live · {online()} players online
        <span class={`s-conn s-conn--${live.status()}`}>{live.status() === "live" ? "" : ` · ${live.status()}`}</span>
      </span>
    </div>
  );
}

export function Panel(props: { title: JSX.Element; right?: JSX.Element; class?: string; children?: JSX.Element; id?: string }) {
  return (
    <section class={`s-panel ${props.class ?? ""}`} id={props.id}>
      <header class="s-panel-head"><span>{props.title}</span><Show when={props.right}><span class="s-panel-right">{props.right}</span></Show></header>
      <div class="s-panel-body">{props.children}</div>
    </section>
  );
}

export function Pager(props: { page: number; pages: number; total: number; per: number; onPage: (p: number) => void }) {
  const from = () => props.total ? props.page * props.per + 1 : 0;
  const to = () => Math.min(props.total, (props.page + 1) * props.per);
  return (
    <div class="s-pager">
      Change page: <button disabled={props.page === 0} onClick={() => props.onPage(props.page - 1)} aria-label="Previous page">&lt;</button>
      <For each={Array.from({ length: props.pages }, (_, i) => i)}>
        {(i) => <button classList={{ on: i === props.page }} onClick={() => props.onPage(i)} aria-current={i === props.page ? "page" : undefined}>{i + 1}</button>}
      </For>
      <button disabled={props.page >= props.pages - 1} onClick={() => props.onPage(props.page + 1)} aria-label="Next page">&gt;</button>
      <span> | Displaying page {props.page + 1} of {Math.max(1, props.pages)}, items {from()} to {to()} of {props.total}.</span>
    </div>
  );
}

export function Derived(props: { children?: JSX.Element; note?: string }) {
  return <span class="s-derived" title={props.note ?? "Derived in your browser from this server's game events; not served by the API."}>{props.children ?? "derived"}<sup>*</sup></span>;
}

export { VerifyMark };
