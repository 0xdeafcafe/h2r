// The "Live" side. Three kinds of screen, like the console dashboard it
// takes after:
//   - the main menu: the orb, an arc of pods and their bars;
//   - list screens (live now, friends, players, game history): a tilted
//     panel of rows, a preview pod and an info plate;
//   - pod screens (a player, sign up, server): one big pod, four small
//     ones around it and an info plate.
// Arrow keys move, Enter (or a) selects, Escape / Backspace (or b) goes
// back; the mouse and touch work too.

import { For, Match, Show, Switch, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity, download } from "../state/identity";
import { liveGames, endedGames, standings, teamStandings, type GameState, type AccountState } from "../state/reducer";
import { playedWith, serviceRecord } from "../state/derive";
import { accountsFile, createAccount, gamertagError, SignUpError } from "../state/account";
import { defaultBase, fetchWellKnown, forget, normalizeBase, recentServers } from "../api/server";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import Emblem from "../components/Emblem";
import { presenceKind } from "../components/bits";
import { ago, clock, titleCase } from "../lib/format";
import { elapsedSeconds, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { outlineFor } from "../state/outlines";
import { now } from "../lib/now";
import Tunnel, { type TunnelAim } from "./Tunnel";
import { BigPod, Glyph, Orb, Plate, Prompts, Tubes, type GlyphName } from "./parts";

type SectionId = "live" | "friends" | "players" | "history" | "signup" | "server";
type ScreenId = "menu" | "list" | "player" | "signup" | "server";

const SECTIONS: Array<{ id: SectionId; label: () => string }> = [
  { id: "live", label: () => "live now" },
  { id: "friends", label: () => "friends" },
  { id: "players", label: () => "players" },
  { id: "history", label: () => "game history" },
  { id: "signup", label: () => (identity.me() ? "profile" : "sign up") },
  { id: "server", label: () => "server" },
];

interface Row { key: string; label: string; meta?: string }
interface Sat { label: string; glyph: GlyphName; act?: () => void; lit?: boolean }

// Kept outside the component: the route remounts it between "/" and
// "/dash/...", and the menu should come back where it was left.
const [menu, setMenu] = createSignal(0);
const listSel: Partial<Record<SectionId, number>> = {};
let booted = false;
let lastScreen: ScreenId | null = null;

const reducedMotion = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

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
  if (g.team_play) return teamStandings(g).slice(0, 2).map((t) => `${teamName(t.team)} ${t.score}`).join(" - ");
  const top = standings(g)[0];
  return top ? `${top.name} ${top.score.score}` : "no score yet";
}

const playerHref = (tag: string) => `/stats/players/${encodeURIComponent(tag)}`;

export default function Dashboard() {
  const params = useParams<{ section?: string }>();
  const [search] = useSearchParams<{ p?: string }>();
  const navigate = useNavigate();

  const section = createMemo((): SectionId | null => SECTIONS.find((s) => s.id === params.section)?.id ?? null);
  const viewed = createMemo((): AccountState | null => {
    const s = section();
    if (s === "signup") { const me = identity.me(); return me ? live.state.accounts[me.xuid] ?? { xuid: me.xuid, gamertag: me.gamertag, created_at: "", online: false, presence: null, last_seen: null } : null; }
    if ((s === "players" || s === "friends") && search.p) return Object.values(live.state.accounts).find((a) => a.gamertag === search.p) ?? null;
    return null;
  });
  const screen = createMemo((): ScreenId => {
    const s = section();
    if (!s) return "menu";
    if (s === "server") return "server";
    if (s === "signup") return identity.me() ? "player" : "signup";
    return viewed() ? "player" : "list";
  });

  createEffect(() => { const s = section(); if (s) setMenu(SECTIONS.findIndex((x) => x.id === s)); });

  // Transitions: the leaving screen plays out, then the route changes and
  // the new screen plays in. "deeper" zooms in, "back" zooms out.
  const [leaving, setLeaving] = createSignal(false);
  const [dir, setDir] = createSignal<"deeper" | "back">("deeper");
  const boot = !booted && !params.section;
  booted = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  function go(href: string, d: "deeper" | "back", replace = false) {
    setDir(d);
    if (reducedMotion()) { navigate(href, { replace }); return; }
    setLeaving(true);
    clearTimeout(timer);
    timer = setTimeout(() => { navigate(href, { replace }); setLeaving(false); }, 190);
  }
  onCleanup(() => clearTimeout(timer));
  createEffect(() => { lastScreen = screen(); });

  const online = createMemo(() => Object.values(live.state.accounts).filter((a) => a.online));
  const friends = createMemo(() => {
    const me = identity.me();
    return me
      ? playedWith(live.state, me.xuid).map((f) => live.state.accounts[f.xuid]).filter((a): a is AccountState => !!a)
          .sort((a, b) => Number(b.online) - Number(a.online))
      : online();
  });

  const rows = createMemo((): Row[] => {
    switch (section()) {
      case "live":
        return liveGames(live.state).map((g) => ({ key: g.game_id, label: titleCase(g.map), meta: `${g.variant ?? g.mode} ${clock(elapsedSeconds(g, now()))}` }));
      case "friends":
        return friends().map((a) => ({ key: a.xuid, label: a.gamertag, meta: presenceText(a) }));
      case "players":
        return Object.values(live.state.accounts)
          .sort((a, b) => Number(b.online) - Number(a.online) || a.gamertag.localeCompare(b.gamertag))
          .map((a) => ({ key: a.xuid, label: a.gamertag, meta: presenceText(a) }));
      case "history":
        return endedGames(live.state, 50).map((g) => ({ key: g.game_id, label: titleCase(g.map), meta: `${g.variant ?? g.mode} ${ago(g.ended_at, now())}` }));
      default:
        return [];
    }
  });

  const [sel, setSelRaw] = createSignal(0);
  const setSel = (i: number) => { setSelRaw(i); const s = section(); if (s) listSel[s] = i; };
  createEffect(() => { const s = section(); setSelRaw(s ? listSel[s] ?? 0 : 0); });
  createEffect(() => { if (screen() === "list" && sel() >= rows().length) setSelRaw(Math.max(0, rows().length - 1)); });

  function openRow(i: number) {
    const r = rows()[i];
    const s = section();
    if (!r || !s) return;
    setSel(i);
    if (s === "live" || s === "history") go(`/stats/games/${r.key}`, "deeper");
    else go(`/dash/${s}?p=${encodeURIComponent(r.label)}`, "deeper");
  }

  // The four small pods of a pod screen.
  const [sat, setSat] = createSignal(0);
  createEffect(() => { screen(); search.p; setSat(0); });
  const sats = createMemo((): Sat[] => {
    switch (screen()) {
      case "player": {
        const a = viewed();
        if (!a) return [];
        const me = identity.me();
        const base: Sat[] = [
          { label: "service record", glyph: "record", act: () => go(playerHref(a.gamertag), "deeper") },
          { label: "game history", glyph: "history", act: () => go(`${playerHref(a.gamertag)}/history`, "deeper") },
        ];
        if (me && me.xuid === a.xuid) {
          return [...base,
            { label: "sign out", glyph: "exit", act: () => { identity.signOut(); } },
            { label: "download for h2", glyph: "download", act: () => download(`h2live-${me.gamertag.replace(/\s+/g, "-")}.json`, accountsFile(me)) },
          ];
        }
        return [...base,
          { label: me ? "compare" : "leaderboard", glyph: "compare", act: () => go(me ? `${playerHref(me.gamertag)}/career?compare=${encodeURIComponent(a.gamertag)}` : "/stats/players", "deeper") },
          { label: "career stats", glyph: "career", act: () => go(`${playerHref(a.gamertag)}/career`, "deeper") },
        ];
      }
      case "signup":
        return [
          { label: "new key", glyph: "key" }, { label: "signed request", glyph: "sign" },
          { label: "h2 import", glyph: "download" }, { label: "this browser", glyph: "store" },
        ];
      case "server": {
        const here = live.server().base;
        const list = [
          ...(here !== defaultBase() ? [{ base: defaultBase(), id: "this site" }] : []),
          ...recentServers().filter((r) => r.base !== here && r.base !== defaultBase()),
        ].slice(0, 4);
        const out: Sat[] = list.map((r) => ({ label: r.id ?? new URL(r.base).host, glyph: "server" as const, act: () => void useServer(r.base) }));
        while (out.length < 4) out.push({ label: `slot ${out.length + 1}`, glyph: "empty" });
        return out;
      }
      default:
        return [];
    }
  });

  const [serverErr, setServerErr] = createSignal<string | null>(null);
  const [serverBusy, setServerBusy] = createSignal(false);
  async function useServer(input: string) {
    setServerErr(null);
    let base: string;
    try { base = normalizeBase(input, location.protocol); } catch (x) { setServerErr((x as Error).message); return; }
    setServerBusy(true);
    try {
      await fetchWellKnown(base);
      await live.useServer(base);
      const u = new URL(location.href);
      if (base === defaultBase()) u.searchParams.delete("server"); else u.searchParams.set("server", base);
      history.replaceState(history.state, "", u);
    } catch (x) {
      setServerErr(`no h2 live server there: ${(x as Error).message}`);
    } finally { setServerBusy(false); }
  }

  function back() {
    (document.activeElement as HTMLElement | null)?.blur();
    const s = section();
    if (screen() === "menu") return;
    if (screen() === "player" && s !== "signup") go(`/dash/${s}`, "back");
    else go("/", "back");
  }

  function select() {
    switch (screen()) {
      case "menu": go(`/dash/${SECTIONS[menu()]!.id}`, "deeper"); break;
      case "list": openRow(sel()); break;
      case "signup": (document.getElementById("dash-signup") as HTMLFormElement | null)?.requestSubmit(); break;
      default: {
        const s = sats()[sat()];
        if (s?.act) s.act();
        else if (screen() === "server") (document.getElementById("dash-server-form") as HTMLFormElement | null)?.requestSubmit();
      }
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey || leaving()) return;
    const typing = (e.target as HTMLElement).closest?.("input, textarea, select");
    if (typing && e.key !== "Escape") return;
    const k = e.key;
    const isBack = k === "Escape" || k === "Backspace" || k === "b" || k === "B";
    const isSelect = k === "Enter" || k === "a" || k === "A";
    const n = SECTIONS.length;
    switch (screen()) {
      case "menu":
        if (k === "ArrowDown") setMenu((menu() + 1) % n);
        else if (k === "ArrowUp") setMenu((menu() + n - 1) % n);
        else if (isSelect || k === "ArrowRight") select();
        else return;
        break;
      case "list":
        if (k === "ArrowDown") setSel(Math.min(rows().length - 1, sel() + 1));
        else if (k === "ArrowUp") setSel(Math.max(0, sel() - 1));
        else if (k === "PageDown") setSel(Math.min(rows().length - 1, sel() + 6));
        else if (k === "PageUp") setSel(Math.max(0, sel() - 6));
        else if (isSelect || k === "ArrowRight") select();
        else if (isBack || k === "ArrowLeft") back();
        else return;
        break;
      default: {
        // Pods sit 0 top left, 1 bottom left, 2 bottom right, 3 top right.
        const i = sat();
        if (isBack) back();
        else if (isSelect) select();
        else if (k === "ArrowLeft" || k === "ArrowRight") setSat(3 - i);
        else if (k === "ArrowUp" || k === "ArrowDown") setSat(i ^ 1);
        else return;
      }
    }
    e.preventDefault();
  }
  onMount(() => document.addEventListener("keydown", onKey));
  onCleanup(() => document.removeEventListener("keydown", onKey));

  const aim = createMemo((): TunnelAim => {
    switch (screen()) {
      case "menu": return { x: 0.36, y: 0.43, dim: 0 };
      case "list": return { x: 0.66, y: 0.38, dim: 1 };
      default: return { x: 0.5, y: 0.36, dim: 1 };
    }
  });

  const title = () => SECTIONS[menu()]!.label();
  const selGame = () => { const r = rows()[sel()]; return r ? live.state.games[r.key] : undefined; };
  const selAccount = () => { const r = rows()[sel()]; return r ? live.state.accounts[r.key] : undefined; };
  const cameFrom = lastScreen;

  return (
    <div
      class="dash dx"
      classList={{ "dx--leaving": leaving(), "dx--boot": boot, "dx--back": dir() === "back" || (cameFrom !== null && cameFrom !== "menu" && screen() === "menu" && !leaving()) }}
      data-screen={screen()}
    >
      <Tunnel aim={aim()} />
      <Tubes />

      <header class="dx-top">
        <span class="dx-brand">h2 live</span>
        <span class="dx-chip"><i class={`dx-led dx-led--${live.status()}`} />{live.wellKnown()?.id ?? new URL(live.server().base).host}</span>
        <Show when={identity.me()}>{(me) => <span class="dx-chip dx-chip--me">{me().gamertag}</span>}</Show>
        <a class="dx-chip dx-chip--link" href="/stats">stats site</a>
      </header>

      <main class="dx-stage">
        <Switch>
          <Match when={screen() === "menu"}>
            <div class="dx-screen dx-menu">
              <Orb />
              <div class="dx-orb-count"><b>{liveGames(live.state).length}</b> live <i /> <b>{online().length}</b> online</div>
              <nav class="dx-arc" aria-label="Dashboard" style={{ "--sel": String(menu()), "--n": String(SECTIONS.length) }}>
                <span class="dx-selbar" aria-hidden="true" />
                <For each={SECTIONS}>
                  {(s, i) => (
                    <button
                      class="dx-item"
                      classList={{ on: menu() === i() }}
                      style={{ "--i": String(i()) }}
                      onMouseEnter={() => setMenu(i())}
                      onFocus={() => setMenu(i())}
                      onClick={() => { setMenu(i()); select(); }}
                      aria-current={menu() === i() ? "true" : undefined}
                    >
                      <span class="dx-pod" aria-hidden="true"><i /></span>
                      <span class="dx-wire" aria-hidden="true" />
                      <span class="dx-bar"><span class="dx-bar-text">{s.label()}</span></span>
                    </button>
                  )}
                </For>
              </nav>
              <Prompts onA={select} />
            </div>
          </Match>

          <Match when={screen() === "list"}>
            <div class="dx-screen dx-list">
              <ListPanel title={title()} rows={rows()} sel={sel()} onSel={setSel} onAct={openRow} empty={emptyText(section())} />
              <div class="dx-preview">
                <BigPod>
                  <Show when={section() === "live" || section() === "history"} fallback={
                    <Show when={selAccount()} fallback={<Glyph name="empty" />}>{(a) => <Emblem seed={a().xuid} size={220} class="dx-emblem" />}</Show>
                  }>
                    <Show when={selGame()} keyed fallback={<Glyph name="empty" />}>{(g) => <GameMap game={g} />}</Show>
                  </Show>
                </BigPod>
              </div>
              <Show when={section() === "live" || section() === "history"} fallback={
                <Show when={selAccount()}>{(a) => <PlayerPlate account={a()} />}</Show>
              }>
                <Show when={selGame()}>{(g) => <GamePlate game={g()} />}</Show>
              </Show>
              <Prompts onA={select} onB={back} />
            </div>
          </Match>

          <Match when={screen() === "player" && viewed()} keyed>
            {(a) => (
              <div class="dx-screen dx-pods">
                <div class="dx-centre"><BigPod wire><Emblem seed={a.xuid} size={220} class="dx-emblem" /></BigPod></div>
                <Sats sats={sats()} sel={sat()} onSel={setSat} />
                <PlayerPlate account={a} main mine={identity.me()?.xuid === a.xuid} />
                <Prompts onA={select} onB={back} />
              </div>
            )}
          </Match>

          <Match when={screen() === "signup"}>
            <div class="dx-screen dx-pods dx-pods--form">
              <div class="dx-centre"><BigPod wire><Glyph name="key" /></BigPod></div>
              <Sats sats={sats()} sel={-1} onSel={() => {}} />
              <SignUpPlate />
              <Prompts onA={select} onB={back} aLabel="create" />
            </div>
          </Match>

          <Match when={screen() === "server"}>
            <div class="dx-screen dx-pods dx-pods--form">
              <div class="dx-centre"><BigPod wire><Glyph name="globe" /></BigPod></div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <ServerPlate onUse={useServer} busy={serverBusy()} err={serverErr()} />
              <Prompts onA={select} onB={back} aLabel="connect" />
            </div>
          </Match>
        </Switch>
      </main>
    </div>
  );
}

function emptyText(s: SectionId | null): string {
  switch (s) {
    case "live": return "no games right now";
    case "friends": return identity.me() ? "play a game to meet people" : "nobody online";
    case "history": return "no finished games yet";
    default: return "nothing here yet";
  }
}

function ListPanel(props: { title: string; rows: Row[]; sel: number; onSel: (i: number) => void; onAct: (i: number) => void; empty: string }) {
  let box!: HTMLDivElement;
  const [first, setFirst] = createSignal(0);
  const [pitch, setPitch] = createSignal(0);
  const [shown, setShown] = createSignal(7);
  const measure = () => {
    const row = box.querySelector<HTMLElement>(".dx-row");
    if (!row) return;
    const p = row.offsetHeight + parseFloat(getComputedStyle(row).marginBottom || "0");
    setPitch(p);
    setShown(Math.max(1, Math.round(box.clientHeight / Math.max(1, p))));
    setFirst(Math.round(box.scrollTop / Math.max(1, p)));
  };
  // Keep the selected row in view, a row's margin from either edge.
  createEffect(() => {
    const i = props.sel;
    const p = pitch();
    if (!p || !box) return;
    const top = box.scrollTop, n = shown();
    let to = top;
    if (i * p < top) to = i * p;
    else if ((i + 1) * p > top + n * p) to = (i + 1 - n) * p;
    if (to !== top) box.scrollTo({ top: to, behavior: reducedMotion() ? "auto" : "smooth" });
  });
  onMount(() => {
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(box);
    onCleanup(() => ro.disconnect());
  });
  createEffect(() => { props.rows.length; queueMicrotask(measure); });
  return (
    <section class="dx-panel" aria-label={props.title}>
      <i class="dx-panel-side" aria-hidden="true" />
      <i class="dx-panel-frame" aria-hidden="true" />
      <header class="dx-panel-head">
        <small>{props.rows.length} {props.rows.length === 1 ? "item" : "items"}</small>
        <h1>{props.title}</h1>
      </header>
      <i class="dx-arrow dx-arrow--up" classList={{ on: first() > 0 }} aria-hidden="true" />
      <div class="dx-rows" ref={box} role="listbox" aria-activedescendant={`dash-item-${props.sel}`} onScroll={measure} style={{ "--sel": String(props.sel) }}>
        <Show when={props.rows.length}><i class="dx-rowsel" aria-hidden="true" /></Show>
        <For each={props.rows} fallback={<div class="dx-row dx-row--empty">{props.empty}</div>}>
          {(r, i) => (
            <div
              id={`dash-item-${i()}`}
              class="dx-row"
              role="option"
              aria-selected={props.sel === i()}
              classList={{ on: props.sel === i() }}
              onMouseEnter={() => props.onSel(i())}
              onClick={() => props.onAct(i())}
            >
              <span class="dx-row-label">{r.label}</span>
              <Show when={r.meta}><span class="dx-row-meta">{r.meta}</span></Show>
            </div>
          )}
        </For>
      </div>
      <i class="dx-arrow dx-arrow--down" classList={{ on: first() + shown() < props.rows.length }} aria-hidden="true" />
    </section>
  );
}

const NO_FRAME: MapFrame = { players: [], markers: [], seen: null };

function GameMap(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  const frame = props.game.status === "live" ? liveFrameSource(props.game.game_id, roster) : () => NO_FRAME;
  return <div class="dx-map"><MapCanvas mini frame={frame} roster={roster} outline={outline} label={`map of ${props.game.map}`} /></div>;
}

function GamePlate(props: { game: GameState }) {
  const g = () => props.game;
  return (
    <Plate small={g().status === "live" ? "live" : "final"} title={g().map}>
      <dl>
        <dt>game</dt><dd>{g().variant ?? g().mode}</dd>
        <dt>players</dt><dd>{Object.keys(g().players).length}</dd>
        <dt>{g().status === "live" ? "leading" : "final"}</dt><dd>{gameLine(g())}</dd>
        <dt>{g().status === "live" ? "clock" : "ended"}</dt><dd>{g().status === "live" ? clock(elapsedSeconds(g(), now())) : ago(g().ended_at, now())}</dd>
      </dl>
    </Plate>
  );
}

const PRESENCE_WORD = { playing: "in a game", online: "online", away: "away", offline: "offline" } as const;

function PlayerPlate(props: { account: AccountState; main?: boolean; mine?: boolean }) {
  const rec = createMemo(() => serviceRecord(live.state, props.account.xuid));
  return (
    <Plate small={props.mine ? "signed in" : PRESENCE_WORD[presenceKind(props.account)]} title={props.account.gamertag} main={props.main}>
      <dl>
        <dt>status</dt><dd>{presenceText(props.account)}</dd>
        <dt>games</dt><dd>{rec().games} <span>played</span> {rec().wins} <span>won</span></dd>
        <dt>kills / deaths</dt><dd>{rec().kills} / {rec().deaths}</dd>
        <dt>rank</dt><dd>{rec().rank}</dd>
        <Show when={props.mine}><dt>key</dt><dd>kept in this browser</dd></Show>
      </dl>
    </Plate>
  );
}

function Sats(props: { sats: Sat[]; sel: number; onSel: (i: number) => void }) {
  return (
    <For each={props.sats}>
      {(s, i) => (
        <button
          class={`dx-sat dx-sat--${i()}`}
          classList={{ on: props.sel === i() && !!s.act, "dx-sat--idle": !s.act }}
          disabled={!s.act}
          onMouseEnter={() => { if (s.act) props.onSel(i()); }}
          onFocus={() => { if (s.act) props.onSel(i()); }}
          onClick={() => s.act?.()}
        >
          <span class="dx-sat-wire" aria-hidden="true" />
          <span class="dx-sat-pod" aria-hidden="true"><Glyph name={s.glyph} /></span>
          <span class="dx-sat-label">{s.label}</span>
        </button>
      )}
    </For>
  );
}

function SignUpPlate() {
  const [tag, setTag] = createSignal("");
  const [invite, setInvite] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
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
    } catch (x) {
      setErr(x instanceof SignUpError ? x.message : `couldn't sign up: ${(x as Error).message}`);
    } finally {
      setBusy(false);
    }
  }
  onMount(() => { if (matchMedia("(pointer: fine)").matches) document.getElementById("dash-gamertag")?.focus({ preventScroll: true }); });
  return (
    <Plate small="new account" title="sign up" main>
      <form id="dash-signup" class="dx-form" onSubmit={submit}>
        <label for="dash-gamertag">gamertag</label>
        <input id="dash-gamertag" value={tag()} maxLength={15} onInput={(e) => setTag(e.currentTarget.value)} autocomplete="off" spellcheck={false} aria-describedby="dash-signup-note" />
        <label for="dash-invite">invite code</label>
        <input id="dash-invite" value={invite()} onInput={(e) => setInvite(e.currentTarget.value)} autocomplete="off" spellcheck={false} placeholder="if the server needs one" />
        <button type="submit" class="dx-go" disabled={busy()}>{busy() ? "signing up" : "create account"}</button>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
        <p id="dash-signup-note" class="dx-note">
          a new key is made here and signs your request. the private key is stored in this browser (IndexedDB, as a JWK, so it can be
          downloaded) for {live.wellKnown()?.id ?? "this server"}.
        </p>
      </form>
    </Plate>
  );
}

function ServerPlate(props: { onUse: (input: string) => Promise<void>; busy: boolean; err: string | null }) {
  const [input, setInput] = createSignal("");
  const [tick, setTick] = createSignal(0);
  const others = () => { tick(); return recentServers().filter((r) => r.base !== live.server().base); };
  return (
    <Plate small={live.status() === "live" ? "connected" : live.status()} title={live.wellKnown()?.id ?? new URL(live.server().base).host} main>
      <form id="dash-server-form" class="dx-form" onSubmit={(e) => { e.preventDefault(); if (input().trim()) void props.onUse(input()).then(() => setInput("")); }}>
        <dl><dt>address</dt><dd>{live.server().base}</dd></dl>
        <label for="dash-server">another server</label>
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
