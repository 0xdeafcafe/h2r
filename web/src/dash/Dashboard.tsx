// The "Live" side. Three kinds of screen, like the console dashboard it
// takes after:
//   - the main menu: the orb and a ring of pods and their bars, which
//     turns to bring the selected one into focus;
//   - list screens (live now, friends, players, game history): a tilted
//     panel of rows, a preview pod and an info plate;
//   - pod screens (settings, a player, sign up, server): one big pod,
//     four small ones around it and an info plate.
// Arrow keys (or the wheel) move, Enter (or a) selects, Escape /
// Backspace (or b) goes back; the mouse and touch work too.

import { For, Match, Show, Switch, createEffect, createMemo, createSignal, on, onCleanup, onMount, untrack } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
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
import { BigPod, Bracket, Glyph, Orb, Plate, Pod, Prompts, Tubes, Vine, type GlyphName } from "./parts";
import { approach, ringPlace } from "./shape";
import { sideOf } from "./cross";
import { inputMode, keyAction, watchGamepads, watchInput, type Action } from "./input";
import { crossTo, crossing } from "./Veil";
import { useMotd } from "../state/motd";
import { motdHeader } from "../lib/motd";
import "../styles/dash-pages.css";
import SocialScreen from "./Social";
import SessionsScreen from "./Sessions";
import "../styles/social.css";

type SectionId = "live" | "sessions" | "friends" | "teams" | "players" | "history" | "settings" | "signup" | "server";
type ScreenId = "menu" | "list" | "player" | "settings" | "signup" | "server" | "social" | "sessions";

const LABELS: Record<SectionId, () => string> = {
  live: () => "live now",
  sessions: () => "find a game",
  friends: () => "friends",
  teams: () => "teams",
  players: () => "players",
  history: () => "game history",
  settings: () => "settings",
  signup: () => (identity.me() ? "profile" : "sign up"),
  server: () => "server",
};
const SECTION_IDS = Object.keys(LABELS) as SectionId[];

/** The main menu: four items, five once signed in (friends, with teams inside).
 *  Sign up / profile and the server live under settings. */
const menuItems = (): SectionId[] => (identity.me() ? ["live", "sessions", "friends", "players", "history", "settings"] : ["live", "sessions", "players", "history", "settings"]);
/** The menu item a section belongs under. Teams live inside friends (inside
 *  players when signed out), as they did in the original Live menu. */
function menuHome(s: SectionId): SectionId {
  if (menuItems().includes(s)) return s;
  if (s === "teams") return identity.me() ? "friends" : "players";
  return s === "friends" ? "players" : "settings";
}

/** The orb's centre on the stage, and how far the ring turns per item (for the orb's skins). */
const ORB_X = 444, ORB_Y = 318, RING_STEP_DEG = 24;

interface Row { key: string; label: string; meta?: string }
interface Sat { label: string; glyph: GlyphName; act?: () => void; lit?: boolean }

// Kept outside the component: the route remounts it between "/" and
// "/dash/...", and the menu should come back where it was left.
const [menuId, setMenuId] = createSignal<SectionId>("live");
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

/** A route segment as typed: the router hands it over still encoded. */
function safeDecode(s: string): string {
  try { return decodeURIComponent(s); } catch { return s; }
}

export default function Dashboard() {
  const params = useParams<{ section?: string; tag?: string }>();
  const navigate = useNavigate();

  const section = createMemo((): SectionId | null => SECTION_IDS.find((s) => s === params.section) ?? null);
  const viewed = createMemo((): AccountState | null => {
    const s = section();
    if (s === "signup") { const me = identity.me(); return me ? live.state.accounts[me.xuid] ?? { xuid: me.xuid, gamertag: me.gamertag, created_at: "", online: false, presence: null, last_seen: null } : null; }
    if ((s === "players" || s === "friends") && params.tag) { const tag = safeDecode(params.tag); return Object.values(live.state.accounts).find((a) => a.gamertag === tag) ?? null; }
    return null;
  });
  const screen = createMemo((): ScreenId => {
    const s = section();
    if (!s) return "menu";
    if (s === "server") return "server";
    if (s === "friends" || s === "teams") return "social";
    if (s === "sessions") return "sessions";
    if (s === "settings") return "settings";
    if (s === "signup") return identity.me() ? "player" : "signup";
    return viewed() ? "player" : "list";
  });

  createEffect(() => { const s = section(); if (s) setMenuId(menuHome(s)); });
  const items = createMemo(menuItems);
  const menu = createMemo(() => Math.max(0, items().indexOf(menuId())));
  const setMenu = (i: number) => { const id = items()[i]; if (id) setMenuId(id); };

  // Transitions: the leaving screen plays out, then the route changes and
  // the new screen plays in. "deeper" zooms in, "back" zooms out.
  const [leaving, setLeaving] = createSignal(false);
  const [dir, setDir] = createSignal<"deeper" | "back">("deeper");
  const cameFrom = lastScreen;
  const boot = !booted && !params.section && !crossing();
  booted = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  function go(href: string, d: "deeper" | "back", replace = false) {
    setDir(d);
    // Off to the stats site: the veil carries it (and plays this screen out).
    if (sideOf(href) === "stats") { crossTo(href, { replace }); return; }
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
    else go(`/dash/${s}/${encodeURIComponent(r.label)}`, "deeper");
  }

  // The four small pods of a pod screen.
  const [sat, setSat] = createSignal(0);
  createEffect(() => { screen(); params.tag; setSat(0); });
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
      case "settings":
        return [
          { label: identity.me() ? "profile" : "sign up", glyph: "key", act: () => go("/dash/signup", "deeper") },
          { label: "stats site", glyph: "record", act: () => go("/stats", "deeper") },
          { label: "event log", glyph: "history", act: () => go("/stats/events", "deeper") },
          { label: "server", glyph: "server", act: () => go("/dash/server", "deeper") },
        ];
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

  // The friends and teams screens (Social.tsx) and the session screens
  // (Sessions.tsx) take their own input.
  let socialAct: ((a: Action) => boolean) | null = null;
  const ownInput = () => screen() === "social" || screen() === "sessions";

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
    else if (s === "signup" || s === "server") go("/dash/settings", "back");
    else if (s === "teams") go(`/dash/${menuHome("teams")}`, "back");
    else go("/", "back");
  }

  function select() {
    switch (screen()) {
      case "social": case "sessions": socialAct?.("select"); break;
      case "menu": go(`/dash/${items()[menu()]!}`, "deeper"); break;
      case "list": openRow(sel()); break;
      case "signup": (document.getElementById("dash-signup") as HTMLFormElement | null)?.requestSubmit(); break;
      default: {
        const s = sats()[sat()];
        if (s?.act) s.act();
        else if (screen() === "server") (document.getElementById("dash-server-form") as HTMLFormElement | null)?.requestSubmit();
      }
    }
  }

  /** One step of input, from the keyboard or a gamepad. Says whether it meant anything here. */
  function act(a: Action): boolean {
    if (leaving()) return false;
    if (ownInput()) return socialAct?.(a) ?? false;
    const n = items().length;
    switch (screen()) {
      case "menu":
        if (a === "down") setMenu((menu() + 1) % n);
        else if (a === "up") setMenu((menu() + n - 1) % n);
        else if (a === "select" || a === "right") select();
        else return false;
        return true;
      case "list":
        if (a === "down") setSel(Math.min(rows().length - 1, sel() + 1));
        else if (a === "up") setSel(Math.max(0, sel() - 1));
        else if (a === "pageDown") setSel(Math.min(rows().length - 1, sel() + 6));
        else if (a === "pageUp") setSel(Math.max(0, sel() - 6));
        else if (a === "select" || a === "right") select();
        else if (a === "back" || a === "left") back();
        else return false;
        return true;
      default: {
        // Pods sit 0 top left, 1 bottom left, 2 bottom right, 3 top right.
        const i = sat();
        if (a === "back") back();
        else if (a === "select") select();
        else if (a === "left" || a === "right") setSat(3 - i);
        else if (a === "up" || a === "down") setSat(i ^ 1);
        else return false;
        return true;
      }
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const typing = (e.target as HTMLElement).closest?.("input, textarea, select");
    if (typing && e.key !== "Escape") return;
    const a = keyAction(e.key);
    if (a && act(a)) e.preventDefault();
  }
  // A gamepad steers the same way; in a form its stick leaves the text caret alone.
  function onPad(a: Action) {
    const typing = document.activeElement?.closest?.("input, textarea, select");
    if (typing && a !== "select" && a !== "back") return;
    act(a);
  }
  onMount(() => {
    const stopInput = watchInput(), stopPads = watchGamepads(onPad);
    onCleanup(() => { stopInput(); stopPads(); });
  });
  onMount(() => document.addEventListener("keydown", onKey));
  // The server's message-of-the-day header, under the orb's count, as the
  // game shows it on its Live main menu. Hidden when there is none.
  const motd = useMotd();
  onCleanup(() => document.removeEventListener("keydown", onKey));

  const aim = createMemo((): TunnelAim => {
    switch (screen()) {
      case "menu": return { x: 0.36, y: 0.44, dim: 0 };
      case "list": return { x: 0.66, y: 0.38, dim: 1 };
      default: return { x: 0.5, y: 0.36, dim: 1 };
    }
  });

  const title = () => LABELS[section() ?? menuId()]();

  // The ring. `turn` is how far it has turned, in items; it eases towards
  // the selected item, leaning a little to whichever one the pointer is on.
  // The first time it shows it spins in from a few items round.
  const [hover, setHover] = createSignal<number | null>(null);
  const lean = () => { const h = hover(); return h === null ? 0 : Math.max(-0.3, Math.min(0.3, (h - menu()) * 0.1)); };
  const [turn, setTurn] = createSignal(untrack(menu));
  const going = () => leaving() || (crossing()?.to === "stats" && crossing()?.phase === "in");
  let raf = 0, last = 0, slow = 0, hoverTimer: ReturnType<typeof setTimeout> | undefined, pick: ReturnType<typeof setTimeout> | undefined, wait: ReturnType<typeof setTimeout> | undefined;
  const frame = (t: number) => {
    // Leaving, the ring swings on past the focus as it fades.
    const to = untrack(menu) + untrack(lean) + (untrack(going) ? 1.4 : 0);
    const next = reducedMotion() ? to : approach(untrack(turn), to, Math.min(50, t - last), t < slow ? 240 : 95);
    last = t;
    setTurn(next);
    raf = next === to ? 0 : requestAnimationFrame(frame);
  };
  const startTurn = () => { if (!raf) { last = performance.now(); raf = requestAnimationFrame(frame); } };
  // Each time the menu shows, the ring swings in: up from below going in,
  // down from above coming back, and a long slow turn the first time.
  let shown = false;
  createEffect(on(screen, (sc) => {
    if (sc !== "menu") { shown = false; return; }
    if (shown) return;
    shown = true;
    clearTimeout(wait);
    if (reducedMotion()) { setTurn(untrack(menu)); return; }
    const first = boot && !slow;
    setTurn(untrack(menu) + (first ? -5 : cameFrom !== null && cameFrom !== "menu" ? 2.2 : -2.2));
    if (first) { slow = performance.now() + 2200; wait = setTimeout(() => { wait = undefined; startTurn(); }, 450); } else startTurn();
  }));
  createEffect(on([menu, lean, going], () => { if (screen() === "menu" && !wait) startTurn(); }, { defer: true }));
  onCleanup(() => { cancelAnimationFrame(raf); clearTimeout(hoverTimer); clearTimeout(pick); clearTimeout(wait); });
  const place = (i: number) => ringPlace(i - turn(), ORB_X, ORB_Y);
  const hoverOn = (i: number) => { clearTimeout(hoverTimer); setHover(i); };
  const hoverOff = () => { clearTimeout(hoverTimer); hoverTimer = setTimeout(() => setHover(null), 260); };
  /** A click: the item swings into focus, then opens. */
  function pickItem(i: number) {
    if (leaving()) return;
    clearTimeout(pick);
    if (menu() === i || reducedMotion()) { setMenu(i); select(); return; }
    setMenu(i);
    pick = setTimeout(select, 230);
  }
  /** The wheel turns the ring a notch at a time. */
  let wheelSum = 0, wheelAt = 0;
  function onWheel(e: WheelEvent) {
    if (screen() !== "menu" || leaving()) return;
    e.preventDefault();
    const t = performance.now();
    if (t - wheelAt > 400) wheelSum = 0;
    wheelSum += e.deltaY + e.deltaX;
    if (Math.abs(wheelSum) < 40 || t - wheelAt < 160) return;
    const to = menu() + Math.sign(wheelSum);
    wheelSum = 0;
    wheelAt = t;
    if (to >= 0 && to < items().length) setMenu(to);
  }
  const selGame = () => { const r = rows()[sel()]; return r ? live.state.games[r.key] : undefined; };
  const selAccount = () => { const r = rows()[sel()]; return r ? live.state.accounts[r.key] : undefined; };

  return (
    <div
      class="dash dx"
      classList={{ "dx--leaving": going(), "dx--boot": boot, "dx--back": dir() === "back" || (cameFrom !== null && cameFrom !== "menu" && screen() === "menu" && !leaving()) }}
      data-screen={screen()}
      data-input={inputMode()}
    >
      <Tunnel aim={aim()} />
      <Tubes />

      <header class="dx-top">
        <span class="dx-brand">h2 live</span>
        <span class="dx-chip dx-chip--server"><i class={`dx-led dx-led--${live.status()}`} />{live.wellKnown()?.id ?? new URL(live.server().base).host}</span>
        <Show when={identity.me()} fallback={<Show when={section() !== "signup"}><a class="dx-chip dx-chip--link dx-chip--join" href="/dash/signup" onClick={(e) => { e.preventDefault(); go("/dash/signup", "deeper"); }}>sign up</a></Show>}>
          {(me) => <a class="dx-chip dx-chip--me" href="/dash/signup" onClick={(e) => { e.preventDefault(); if (section() !== "signup") go("/dash/signup", "deeper"); }}>{me().gamertag}</a>}
        </Show>
        <a class="dx-chip dx-chip--link" href="/stats">stats site</a>
      </header>

      <main class="dx-stage">
        <Switch>
          <Match when={screen() === "menu"}>
            <div class="dx-screen dx-menu" onWheel={onWheel}>
              <Orb turn={-turn() * RING_STEP_DEG} />
              <div class="dx-orb-count"><b>{liveGames(live.state).length}</b> live <i /> <b>{online().length}</b> online</div>
              <Show when={motdHeader(motd())}>{(h) => <p class="dx-motd">{h()}</p>}</Show>
              <nav class="dx-arc" aria-label="Dashboard" style={{ "--n": String(items().length) }}>
                <For each={items()}>
                  {(id, i) => {
                    const at = createMemo(() => place(i()));
                    return (
                      <button
                        class="dx-item"
                        classList={{ on: menu() === i(), hov: hover() === i(), "dx-item--far": at().opacity < 0.12 }}
                        style={{ "--i": String(i()), "--x": at().x.toFixed(2), "--y": at().y.toFixed(2), "--s": at().scale.toFixed(4), "--o": at().opacity.toFixed(3), "--dr": at().drop.toFixed(2), "--lit": at().lit.toFixed(3) }}
                        onMouseEnter={() => hoverOn(i())}
                        onMouseLeave={hoverOff}
                        onFocus={() => setMenu(i())}
                        onClick={() => pickItem(i())}
                        aria-current={menu() === i() ? "true" : undefined}
                      >
                        <Vine />
                        <Pod seed={i()} />
                        <span class="dx-bar"><Bracket /><i class="dx-bar-lit" aria-hidden="true" /><span class="dx-bar-text">{LABELS[id]()}</span></span>
                      </button>
                    );
                  }}
                </For>
              </nav>
              <Prompts onA={select} />
            </div>
          </Match>

          <Match when={screen() === "list"}>
            <div class="dx-screen dx-list dx-pages">
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
              <div class="dx-screen dx-pods dx-pages">
                <div class="dx-centre"><BigPod wire><Emblem seed={a.xuid} size={220} class="dx-emblem" /></BigPod></div>
                <Sats sats={sats()} sel={sat()} onSel={setSat} />
                <PlayerPlate account={a} main mine={identity.me()?.xuid === a.xuid} />
                <Prompts onA={select} onB={back} />
              </div>
            )}
          </Match>

          <Match when={screen() === "settings"}>
            <div class="dx-screen dx-pods dx-pages">
              <div class="dx-centre"><BigPod wire><Glyph name="gear" /></BigPod></div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <Plate small="h2 live" title="settings" main>
                <dl>
                  <dt>server</dt><dd>{live.wellKnown()?.id ?? new URL(live.server().base).host}</dd>
                  <dt>link</dt><dd>{live.status() === "live" ? "connected" : live.status()}</dd>
                  <dt>account</dt><dd>{identity.me()?.gamertag ?? "not signed in"}</dd>
                  <dt>online</dt><dd>{online().length} <span>players</span> {liveGames(live.state).length} <span>games</span></dd>
                </dl>
              </Plate>
              <Prompts onA={select} onB={back} />
            </div>
          </Match>

          <Match when={screen() === "signup"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages">
              <div class="dx-centre"><BigPod wire><Glyph name="key" /></BigPod></div>
              <Sats sats={sats()} sel={-1} onSel={() => {}} />
              <SignUpPlate />
              <Prompts onA={select} onB={back} aLabel="create" />
            </div>
          </Match>

          <Match when={screen() === "social"}>
            <SocialScreen section={section() as "friends" | "teams"} tag={params.tag ? safeDecode(params.tag) : undefined} go={go} back={back} bind={(f) => { socialAct = f; }} />
          </Match>

          <Match when={screen() === "sessions"}>
            <SessionsScreen id={params.tag ? safeDecode(params.tag) : undefined} go={go} back={back} bind={(f) => { socialAct = f; }} />
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

export function ListPanel(props: { title: string; rows: Row[]; sel: number; onSel: (i: number) => void; onAct: (i: number) => void; empty: string }) {
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
      <div class="dx-rows" ref={box} role="listbox" tabindex="0" aria-label={props.title} aria-activedescendant={props.rows.length ? `dash-item-${props.sel}` : undefined} onScroll={measure}>
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
      <div class="dx-list-foot"><span>h2 live</span><span>↑ ↓ browse</span></div>
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
      <p class="dx-signup-intro">Choose your name. Join the game.</p>
      <form id="dash-signup" class="dx-form" onSubmit={submit}>
        <label for="dash-gamertag">gamertag</label>
        <input id="dash-gamertag" value={tag()} maxLength={15} onInput={(e) => setTag(e.currentTarget.value)} autocomplete="off" spellcheck={false} placeholder="your name, up to 15 characters" aria-describedby="dash-signup-note" />
        <label for="dash-invite">invite code</label>
        <input id="dash-invite" value={invite()} onInput={(e) => setInvite(e.currentTarget.value)} autocomplete="off" spellcheck={false} placeholder="if the server needs one" />
        <button type="submit" class="dx-go" disabled={busy()}>{busy() ? "signing up" : "create account"}</button>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
        <p id="dash-signup-note" class="dx-note">
          Your account key is saved in this browser for {live.wellKnown()?.id ?? "this server"}. Anyone with access to this browser profile can use it.
          After signing up, choose “download for h2” to take your account into the game.
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
