// The "Live" side: a glowing orb, a menu arc of pods and bars, and a
// bezelled panel for the selected item. Arrow keys move, Enter is A,
// Escape/Backspace is B; the mouse works too.

import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity, download } from "../state/identity";
import { liveGames, endedGames, standings, teamStandings, type GameState, type AccountState } from "../state/reducer";
import { playedWith, serviceRecord } from "../state/derive";
import { accountsFile, createAccount, gamertagError, SignUpError, type LocalAccount } from "../state/account";
import { defaultBase, fetchWellKnown, forget, normalizeBase, recentServers } from "../api/server";
import MapCanvas from "../components/MapCanvas";
import Emblem from "../components/Emblem";
import { presenceKind } from "../components/bits";
import { ago, clock, titleCase } from "../lib/format";
import { elapsedSeconds, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { outlineFor } from "../state/outlines";
import { now } from "../lib/now";

type SectionId = "live" | "friends" | "players" | "history" | "signup" | "server";

interface Item { key: string; label: string; meta?: string; href?: string; act?: () => void }

const SECTIONS: Array<{ id: SectionId; label: () => string }> = [
  { id: "live", label: () => "live now" },
  { id: "friends", label: () => "friends" },
  { id: "players", label: () => "players" },
  { id: "history", label: () => "game history" },
  { id: "signup", label: () => (identity.me() ? "profile" : "sign up") },
  { id: "server", label: () => "server" },
];

function presenceText(a: AccountState | undefined): string {
  switch (presenceKind(a)) {
    case "playing": {
      const g = a?.presence?.game_id ? live.state.games[a.presence.game_id] : undefined;
      return g ? `playing ${g.map}` : "in a game";
    }
    case "online": return "online";
    case "away": return "away";
    default: return a?.last_seen ? `seen ${ago(a.last_seen, now())}` : "offline";
  }
}

function gameLine(g: GameState): string {
  if (g.team_play) return teamStandings(g).slice(0, 2).map((t) => `${teamName(t.team).toLowerCase()} ${t.score}`).join(" · ");
  const top = standings(g)[0];
  return top ? `${top.name} ${top.score.score}` : "";
}

export default function Dashboard() {
  const params = useParams<{ section?: string }>();
  const navigate = useNavigate();
  const start = Math.max(0, SECTIONS.findIndex((s) => s.id === params.section));
  const [menu, setMenu] = createSignal(start);
  const [focus, setFocus] = createSignal<"menu" | "panel">(params.section ? "panel" : "menu");
  const [sel, setSel] = createSignal(0);
  const section = () => SECTIONS[menu()]!.id;

  createEffect(() => {
    const id = section();
    if (params.section !== id && (params.section || menu() !== 0)) navigate(`/dash/${id}`, { replace: true });
  });

  const online = createMemo(() => Object.values(live.state.accounts).filter((a) => a.online));

  const items = createMemo((): Item[] => {
    switch (section()) {
      case "live":
        return liveGames(live.state).map((g) => ({
          key: g.game_id, label: titleCase(g.map), meta: `${(g.variant ?? g.mode).toLowerCase()} · ${clock(elapsedSeconds(g, now()))}`,
          href: `/stats/games/${g.game_id}`,
        }));
      case "friends": {
        const me = identity.me();
        const list = me
          ? playedWith(live.state, me.xuid).map((f) => live.state.accounts[f.xuid]).filter((a): a is AccountState => !!a)
              .sort((a, b) => Number(b.online) - Number(a.online))
          : online();
        return list.map((a) => ({ key: a.xuid, label: a.gamertag, meta: presenceText(a), href: `/stats/players/${encodeURIComponent(a.gamertag)}` }));
      }
      case "players":
        return Object.values(live.state.accounts)
          .sort((a, b) => Number(b.online) - Number(a.online) || a.gamertag.localeCompare(b.gamertag))
          .map((a) => ({ key: a.xuid, label: a.gamertag, meta: presenceText(a), href: `/stats/players/${encodeURIComponent(a.gamertag)}` }));
      case "history":
        return endedGames(live.state, 50).map((g) => ({
          key: g.game_id, label: titleCase(g.map), meta: `${(g.variant ?? g.mode).toLowerCase()} · ${ago(g.ended_at, now())}`,
          href: `/stats/games/${g.game_id}`,
        }));
      case "signup": {
        const me = identity.me();
        if (!me) return [];
        return [
          { key: "record", label: "service record", href: `/stats/players/${encodeURIComponent(me.gamertag)}` },
          { key: "download", label: "download for h2", act: () => download(`h2live-${me.gamertag.replace(/\s+/g, "-")}.json`, accountsFile(me)) },
          { key: "signout", label: "sign out", act: () => identity.signOut() },
        ];
      }
      case "server":
        return [
          ...(live.server().base !== defaultBase() ? [{ key: "here", label: "this site", meta: new URL(defaultBase()).host, act: () => void live.useServer(defaultBase()) }] : []),
          ...recentServers().filter((r) => r.base !== live.server().base).map((r) => ({
            key: r.base, label: r.id ?? new URL(r.base).host, meta: r.base, act: () => void live.useServer(r.base),
          })),
        ];
    }
  });

  createEffect(() => { if (sel() >= items().length) setSel(Math.max(0, items().length - 1)); });

  function activate(i: number) {
    const it = items()[i];
    if (!it) return;
    if (it.href) navigate(it.href);
    else it.act?.();
  }

  function onKey(e: KeyboardEvent) {
    const typing = (e.target as HTMLElement).closest("input, textarea, select");
    if (typing && e.key !== "Escape") return;
    const k = e.key;
    if (focus() === "menu") {
      if (k === "ArrowDown") { setMenu((menu() + 1) % SECTIONS.length); setSel(0); }
      else if (k === "ArrowUp") { setMenu((menu() + SECTIONS.length - 1) % SECTIONS.length); setSel(0); }
      else if (k === "ArrowRight" || k === "Enter" || k === "a") {
        setFocus("panel");
        if (section() === "signup" && !identity.me()) queueMicrotask(() => document.getElementById("dash-gamertag")?.focus());
        if (section() === "server") queueMicrotask(() => document.getElementById("dash-server")?.focus());
      } else return;
    } else {
      if (k === "ArrowDown") setSel(Math.min(items().length - 1, sel() + 1));
      else if (k === "ArrowUp") setSel(Math.max(0, sel() - 1));
      else if (k === "Enter" || k === "a") activate(sel());
      else if (k === "Escape" || k === "Backspace" || k === "ArrowLeft" || k === "b") {
        (document.activeElement as HTMLElement | null)?.blur();
        setFocus("menu");
      } else return;
    }
    e.preventDefault();
  }
  onMount(() => document.addEventListener("keydown", onKey));
  onCleanup(() => document.removeEventListener("keydown", onKey));

  return (
    <div class="dash">
      <Tunnel />
      <header class="dash-top">
        <span class="dash-brand">h2 live</span>
        <span class="dash-server"><span class={`dash-led dash-led--${live.status()}`} />{live.wellKnown()?.id ?? new URL(live.server().base).host}</span>
        <Show when={identity.me()} fallback={<span class="dash-me">not signed in</span>}>
          {(me) => <span class="dash-me">signed in as <b>{me().gamertag}</b></span>}
        </Show>
        <a class="dash-stats-link" href="/stats">stats site ›</a>
      </header>

      <div class="dash-stage">
        <div class="orb-wrap" aria-hidden="true">
          <div class="orb-shell" />
          <div class="orb" />
          <div class="orb-count">
            <b>{liveGames(live.state).length}</b> live · <b>{online().length}</b> online
          </div>
        </div>

        <nav class="arc" aria-label="Dashboard">
          <For each={SECTIONS}>
            {(s, i) => (
              <button
                class="arc-item"
                classList={{ "arc-item--on": menu() === i(), "arc-item--focus": menu() === i() && focus() === "menu" }}
                style={{ "--i": String(i()), "--n": String(SECTIONS.length) }}
                onMouseEnter={() => { if (focus() === "menu") { setMenu(i()); setSel(0); } }}
                onClick={() => { setMenu(i()); setSel(0); setFocus("panel"); }}
                aria-current={menu() === i() ? "page" : undefined}
              >
                <span class="pod" />
                <span class="bar">{s.label()}</span>
              </button>
            )}
          </For>
        </nav>

        <section class="bezel" classList={{ "bezel--focus": focus() === "panel" }} aria-label={SECTIONS[menu()]!.label()}>
          <div class="bezel-tab">{SECTIONS[menu()]!.label()}</div>
          <div class="bezel-body">
            <Show when={section() === "signup" && !identity.me()}>
              <SignUp />
            </Show>
            <Show when={section() === "server"}>
              <ServerForm />
            </Show>
            <Show when={section() === "signup" && identity.me()}>
              {(_) => <Profile me={identity.me()!} />}
            </Show>
            <Show when={items().length || (section() !== "signup" && section() !== "server")}>
              <ListBox items={items()} sel={sel()} focused={focus() === "panel"} onSel={setSel} onAct={activate} empty={emptyText(section())} />
            </Show>
            <Detail section={section()} item={items()[sel()]} />
          </div>
        </section>
      </div>

      <footer class="prompts">
        <span class="prompt"><span class="btn-a">a</span> select</span>
        <span class="prompt"><span class="btn-b">b</span> back</span>
      </footer>
    </div>
  );
}

function emptyText(s: SectionId): string {
  switch (s) {
    case "live": return "no games right now";
    case "friends": return identity.me() ? "play a game to meet people" : "nobody online";
    case "history": return "no finished games yet";
    default: return "nothing here yet";
  }
}

function ListBox(props: { items: Item[]; sel: number; focused: boolean; onSel: (i: number) => void; onAct: (i: number) => void; empty: string }) {
  let box!: HTMLUListElement;
  const VISIBLE = 7;
  const first = () => Math.max(0, Math.min(props.sel - Math.floor(VISIBLE / 2), props.items.length - VISIBLE));
  createEffect(() => {
    const el = box?.children[props.sel] as HTMLElement | undefined;
    el?.scrollIntoView({ block: "nearest" });
  });
  return (
    <div class="listbox">
      <span class="scroll-arrow scroll-arrow--up" classList={{ on: first() > 0 }} aria-hidden="true" />
      <ul ref={box} role="listbox" aria-activedescendant={`dash-item-${props.sel}`}>
        <For each={props.items} fallback={<li class="listbox-empty">{props.empty}</li>}>
          {(it, i) => (
            <li
              id={`dash-item-${i()}`}
              role="option"
              aria-selected={props.sel === i()}
              classList={{ on: props.sel === i(), focused: props.focused && props.sel === i() }}
              onMouseEnter={() => props.onSel(i())}
              onClick={() => props.onAct(i())}
            >
              <span class="li-label">{it.label}</span>
              <Show when={it.meta}><span class="li-meta">{it.meta}</span></Show>
            </li>
          )}
        </For>
      </ul>
      <span class="scroll-arrow scroll-arrow--down" classList={{ on: first() + VISIBLE < props.items.length }} aria-hidden="true" />
    </div>
  );
}

function Detail(props: { section: SectionId; item: Item | undefined }): JSX.Element {
  return (
    <div class="dash-detail">
      <Show when={props.section === "live" || props.section === "history"}>
        <Show when={props.item && live.state.games[props.item.key]}>
          {(g) => <GamePeek game={g()} />}
        </Show>
      </Show>
      <Show when={props.section === "players" || props.section === "friends"}>
        <Show when={props.item && live.state.accounts[props.item.key]}>
          {(a) => <PlayerPeek account={a()} />}
        </Show>
      </Show>
    </div>
  );
}

function GamePeek(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  return (
    <div class="peek">
      <Show when={props.game.status === "live"} fallback={<div class="peek-ended">final · {gameLine(props.game)}</div>}>
        <div class="peek-map"><MapCanvas mini frame={liveFrameSource(props.game.game_id, roster)} roster={roster} outline={outline} label={`map of ${props.game.map}`} /></div>
      </Show>
      <div class="peek-lines">
        <span>{Object.keys(props.game.players).length} players</span>
        <span>{gameLine(props.game)}</span>
      </div>
    </div>
  );
}

function PlayerPeek(props: { account: AccountState }) {
  const rec = createMemo(() => serviceRecord(live.state, props.account.xuid));
  return (
    <div class="peek peek--player">
      <Emblem seed={props.account.xuid} size={64} />
      <div class="peek-lines">
        <span>{rec().games} games · {rec().wins} wins</span>
        <span>k/d {rec().kd === null ? `${rec().kills}/0` : rec().kd!.toFixed(2)}</span>
        <span>{rec().rank.toLowerCase()}</span>
      </div>
    </div>
  );
}

function SignUp() {
  const [tag, setTag] = createSignal("");
  const [invite, setInvite] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const [made, setMade] = createSignal<LocalAccount | null>(null);
  async function submit(e: Event) {
    e.preventDefault();
    setErr(null);
    const bad = gamertagError(tag().trim());
    if (bad) { setErr(`gamertag: ${bad}`); return; }
    setBusy(true);
    try {
      const wk = live.wellKnown() ?? (await fetchWellKnown(live.server().base));
      const a = await createAccount(live.server().api, live.server().base, wk.id, tag().trim(), invite().trim() || undefined);
      await identity.add(a);
      setMade(a);
    } catch (x) {
      setErr(x instanceof SignUpError ? x.message : `couldn't sign up: ${(x as Error).message}`);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Show when={!made()} fallback={<p class="dash-note">welcome, {made()!.gamertag}. your key lives in this browser; download it for h2 below.</p>}>
      <form class="dash-form" onSubmit={submit}>
        <label for="dash-gamertag">gamertag</label>
        <input id="dash-gamertag" value={tag()} maxLength={15} onInput={(e) => setTag(e.currentTarget.value)} autocomplete="off" aria-describedby="dash-signup-note" />
        <label for="dash-invite">invite code <small>(if the server needs one)</small></label>
        <input id="dash-invite" value={invite()} onInput={(e) => setInvite(e.currentTarget.value)} autocomplete="off" />
        <button type="submit" class="dash-button" disabled={busy()}><span class="btn-a">a</span> {busy() ? "signing up…" : "create account"}</button>
        <Show when={err()}><p class="dash-error" role="alert">{err()}</p></Show>
        <p id="dash-signup-note" class="dash-note">
          a new key is made here and signs your request. the private key is stored in this browser (IndexedDB, as a JWK, so it can be
          downloaded) for {live.wellKnown()?.id ?? "this server"}.
        </p>
      </form>
    </Show>
  );
}

function Profile(props: { me: LocalAccount }) {
  return (
    <div class="peek peek--player">
      <Emblem seed={props.me.xuid} size={64} />
      <div class="peek-lines">
        <span><b>{props.me.gamertag}</b></span>
        <span>{props.me.id}</span>
        <span class="dash-note">import into h2 with <code>h2 live account import &lt;file&gt;</code></span>
      </div>
    </div>
  );
}

function ServerForm() {
  const [input, setInput] = createSignal("");
  const [err, setErr] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  async function go(e: Event) {
    e.preventDefault();
    setErr(null);
    let base: string;
    try { base = normalizeBase(input(), location.protocol); } catch (x) { setErr((x as Error).message); return; }
    setBusy(true);
    try {
      await fetchWellKnown(base);
      await live.useServer(base);
      const u = new URL(location.href);
      if (base === defaultBase()) u.searchParams.delete("server"); else u.searchParams.set("server", base);
      history.replaceState(history.state, "", u);
      setInput("");
    } catch (x) {
      setErr(`no h2 live server there: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }
  return (
    <form class="dash-form" onSubmit={go}>
      <div class="dash-note">connected to <b>{live.wellKnown()?.id ?? live.server().base}</b> · {live.server().base}</div>
      <label for="dash-server">server address</label>
      <input id="dash-server" placeholder="live.example.net" value={input()} onInput={(e) => setInput(e.currentTarget.value)} autocomplete="off" />
      <button type="submit" class="dash-button" disabled={busy() || !input().trim()}><span class="btn-a">a</span> {busy() ? "connecting…" : "connect"}</button>
      <Show when={err()}><p class="dash-error" role="alert">{err()}</p></Show>
      <Show when={recentServers().length}><button type="button" class="dash-link" onClick={() => { for (const r of recentServers()) if (r.base !== live.server().base) forget(r.base); }}>forget other servers</button></Show>
    </form>
  );
}

/** The wireframe tunnel behind everything: rings and spokes. */
function Tunnel() {
  const rings = Array.from({ length: 14 }, (_, i) => i);
  const spokes = Array.from({ length: 28 }, (_, i) => i);
  return (
    <svg class="tunnel" viewBox="-800 -500 1600 1000" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
      <defs>
        <radialGradient id="tunnel-fade" cx="0" cy="0" r="0.75">
          <stop offset="0" stop-color="#000" stop-opacity="0.9" />
          <stop offset="0.35" stop-color="#000" stop-opacity="0" />
        </radialGradient>
      </defs>
      <g class="tunnel-grid" transform="translate(-260 0)">
        <For each={rings}>{(i) => {
          const r = 40 * Math.pow(1.32, i);
          return <ellipse cx="0" cy="0" rx={r * 1.25} ry={r} />;
        }}</For>
        <For each={spokes}>{(i) => {
          const a = (i / spokes.length) * Math.PI * 2;
          return <line x1={Math.cos(a) * 50} y1={Math.sin(a) * 40} x2={Math.cos(a) * 2400} y2={Math.sin(a) * 1900} />;
        }}</For>
      </g>
      <rect x="-800" y="-500" width="1600" height="1000" fill="url(#tunnel-fade)" />
    </svg>
  );
}
