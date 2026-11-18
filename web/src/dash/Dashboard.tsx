// The "Live" side. Three kinds of screen, like the console dashboard it
// takes after:
//   - the main menu: the orb and a ring of pods and their bars, which
//     turns to bring the selected one into focus;
//   - list screens (live now, friends, players, game history): a tilted
//     panel of rows, a preview pod and an info plate;
//   - pod screens (settings, a player, sign up, recovery, server, invites):
//     one big pod, four small ones around it and an info plate.
// Arrow keys (or the wheel) move, Enter (or a) selects, Escape /
// Backspace (or b) goes back; the mouse and touch work too.

import { gameMapTitle } from "../state/maps";
import { For, Match, Show, Switch, createEffect, createMemo, createSignal, on, onCleanup, onMount, untrack } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity, download } from "../state/identity";
import { liveGames, endedGames, standings, teamStandings, type GameState, type AccountState } from "../state/reducer";
import { OUTCOME_WORDS, campaignOutcome, campaignRecord, playedWith, serviceRecord } from "../state/derive";
import { campaignLabel, difficultyName, gameTypeLabel, gameTypeLine } from "../state/gametype";

/** A list row's type: "Custom Game", "Matchmaking: <playlist>", or the
 *  campaign label with its difficulty. The variant and when it ended are
 *  on the plate beside the list. */
function rowType(g: GameState): string {
  return isCampaign(g) ? [campaignLabel(g), difficultyName(g.difficulty)].filter(Boolean).join(" · ") : gameTypeLabel(g);
}
import { isCampaign } from "../state/reducer";
import { mapPicture } from "../components/MapPicture";
import { accountsFile, gamertagError, signUp, SignUpError } from "../state/account";
import { AccountPlate, RecoverPlate, RecoveryCode, createRecovery, keepFreshRecovery } from "./Recovery";
import "../styles/dash-recovery.css";
import { defaultBase, fetchWellKnown, normalizeBase, recentServers } from "../api/server";
import { AccountsPlate, ConnectPlate, ImportPlate, InvitesPlate, ServerPlate, createServerInfo, type InviteControls } from "./Server";
import { ReportsQueue } from "./Reports";
import { BanControl, BanNotice, BansList } from "./Bans";
import { MuteButton, MutedList } from "./Mutes";
import { adminState } from "../admin/state";
import type { BladeControls } from "./Blades";
import { fetchServerFacts, serverPods } from "../admin/server";
import "../styles/dash-server.css";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import Emblem from "../components/Emblem";
import { presenceKind } from "../components/bits";
import { ago, clock } from "../lib/format";
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
import GamesScreen from "./Games";
import "../styles/social.css";
import { lockedHint, menuFor, type SectionId } from "./menu";

type ScreenId = "menu" | "games" | "list" | "player" | "settings" | "signup" | "recovery" | "recover" | "server" | "social" | "sessions";

const LABELS: Record<SectionId, () => string> = {
  live: () => "live now",
  games: () => "games",
  sessions: () => "find a game",
  friends: () => "friends",
  clans: () => "clans",
  players: () => "players",
  history: () => "game history",
  settings: () => "settings",
  signup: () => (identity.me() ? "profile" : "create account"),
  server: () => "server",
  signin: () => "sign in",
  recover: () => "recover account",
  account: () => "account",
};
const SECTION_IDS = Object.keys(LABELS) as SectionId[];

const menuItems = (): SectionId[] => menuFor(!!identity.me());
/** The menu item a section belongs under. Clans live inside friends (inside
 *  players when signed out), as they did in the original Live menu. */
function menuHome(s: SectionId): SectionId {
  if (menuItems().includes(s)) return s;
  if (s === "clans") return identity.me() ? "friends" : "players";
  return s === "friends" ? "players" : "settings";
}

/** The orb's centre on the stage, and how far the ring turns per item (for the orb's skins). */
const ORB_X = 444, ORB_Y = 318, RING_STEP_DEG = 24;

interface Row { key: string; label: string; meta?: string }
interface Sat { label: string; glyph: GlyphName; act?: () => void; lit?: boolean }

// Kept outside the component: the route remounts it between "/" and
// "/dash/...", and the menu should come back where it was left.
const [menuId, setMenuId] = createSignal<SectionId>("live");
let menuTouched = false;
const listSel: Partial<Record<SectionId, number>> = {};
let booted = false;
let lastScreen: ScreenId | null = null;

const reducedMotion = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

function presenceText(a: AccountState | undefined): string {
  switch (presenceKind(a)) {
    case "playing": {
      const g = a?.presence?.game_id ? live.state.games[a.presence.game_id] : undefined;
      return g ? `playing ${gameMapTitle(g)}` : "in a game";
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
    if (s === "games") return "games";
    if (s === "server") return "server";
    if (s === "friends" || s === "clans") return "social";
    if (s === "sessions") return "sessions";
    if (s === "settings") return "settings";
    // /dash/recover (and the older /dash/signup/recover): a new device takes
    // an account over; /dash/account (and /dash/signup/recovery): the
    // signed-in account. Signed out, /dash/account is the sign-in list.
    if (s === "recover" || (s === "signup" && params.tag === "recover")) return "recover";
    if ((s === "account" || (s === "signup" && params.tag === "recovery")) && identity.me()) return "recovery";
    if (s === "signin" || s === "account") return "list";
    if (s === "signup") return identity.me() ? "player" : "signup";
    return viewed() ? "player" : "list";
  });

  createEffect(() => { const s = section(); if (s) { menuTouched = true; setMenuId(menuHome(s)); } });
  const items = createMemo(menuItems);
  // Until someone moves it, the focus sits on the first item: "sign in"
  // signed out, "live now" signed in (the account loads a moment later).
  createEffect(on(items, (it) => { if (!menuTouched) setMenuId(it[0]!); }));
  const menu = createMemo(() => Math.max(0, items().indexOf(menuId())));
  const setMenu = (i: number) => { const id = items()[i]; if (id) { menuTouched = true; setMenuId(id); } };

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
        return liveGames(live.state).map((g) => ({ key: g.game_id, label: gameMapTitle(g), meta: `${rowType(g)} ${clock(elapsedSeconds(g, now()))}` }));
      case "friends":
        return friends().map((a) => ({ key: a.xuid, label: a.gamertag, meta: presenceText(a) }));
      case "players":
        return Object.values(live.state.accounts)
          .sort((a, b) => Number(b.online) - Number(a.online) || a.gamertag.localeCompare(b.gamertag))
          .map((a) => ({ key: a.xuid, label: a.gamertag, meta: presenceText(a) }));
      case "history":
        return endedGames(live.state, 50).map((g) => ({ key: g.game_id, label: gameMapTitle(g), meta: rowType(g) }));
      case "signin": case "account": {
        // The accounts this browser keeps for this server, then the other ways in.
        const me = identity.me();
        return [
          ...identity.accounts().map((a) => ({ key: a.xuid, label: a.gamertag, meta: me?.xuid === a.xuid ? "signed in" : "kept here" })),
          { key: "#recover", label: "recover account", meta: "gamertag and code" },
          { key: "#create", label: "create account", meta: "new gamertag" },
          { key: "#import", label: "import account", meta: "account file" },
        ];
      }
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
    if (s === "signin" || s === "account") {
      if (r.key === "#recover") go("/dash/recover", "deeper");
      else if (r.key === "#create") go("/dash/signup", "deeper");
      else if (r.key === "#import") go("/dash/server/import", "deeper");
      else {
        const a = identity.accounts().find((x) => x.xuid === r.key);
        if (a) { identity.signIn(a); go("/", "back"); }
      }
      return;
    }
    if (s === "live" || s === "history") go(`/stats/games/${r.key}`, "deeper");
    else go(`/dash/${s}/${encodeURIComponent(r.label)}`, "deeper");
  }

  // The server screens (Server.tsx): /dash/server, and under it invites,
  // accounts, import and connect.
  const serverSub = () => (section() === "server" ? params.tag ?? null : null);
  const serverInfo = createServerInfo(() => section() === "server");
  // The plate's blades on the server screen in view (each binds its own; the
  // screen it was bound on is kept so a stale one never takes input).
  const [plateCtl, setPlateCtl] = createSignal<{ sub: string; ctl: BladeControls & Partial<Pick<InviteControls, "create" | "copy">> } | null>(null);
  const bindPlate = (sub: string) => (ctl: BladeControls) => setPlateCtl({ sub, ctl });
  const blades = () => { const p = plateCtl(); return p && p.sub === serverSub() ? p.ctl : null; };
  const recovery = createRecovery(() => screen() === "recovery");

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
          { label: "games", glyph: "disc", act: () => go("/dash/games", "deeper") },
          { label: "event log", glyph: "history", act: () => go("/stats/events", "deeper") },
          { label: "server", glyph: "server", act: () => go("/dash/server", "deeper") },
        ];
      case "recovery": {
        const ready = recovery.phase() === "ready";
        return [
          { label: "copy code", glyph: "sign", act: ready ? () => void recovery.copyCode() : undefined },
          { label: recovery.showSetup() ? "hide setup" : "authenticator setup", glyph: "download", act: ready ? () => recovery.toggleSetup() : undefined },
          {
            label: recovery.confirming() ? "confirm reset" : ready ? "reset recovery" : "set up recovery", glyph: "gear",
            act: recovery.phase() === "ready" || recovery.phase() === "none" ? () => recovery.setupOrReset() : undefined,
          },
          { label: "sign out", glyph: "exit", act: () => { identity.signOut(); go("/", "back"); } },
        ];
      }
      case "recover":
        return [
          { label: "new key", glyph: "key" }, { label: "your code", glyph: "sign" },
          { label: "this browser", glyph: "store" }, { label: "create account", glyph: "career", act: () => go("/dash/signup", "deeper") },
        ];
      case "signup":
        return [
          { label: "new key", glyph: "key" }, { label: "signed request", glyph: "sign" },
          { label: "h2 import", glyph: "download" }, { label: "this browser", glyph: "store" },
        ];
      case "server": {
        const sub = serverSub();
        if (sub !== "connect") {
          const view = sub === "invites" || sub === "accounts" || sub === "import" || sub === "reports" || sub === "bans" ? sub : "overview";
          return serverPods(view, serverInfo.admin()).map((p): Sat => ({
            label: p.label, glyph: p.glyph,
            act: p.run ? () => (p.run === "create" ? blades()?.create?.() : blades()?.copy?.()) : () => go(p.to!, p.back ? "back" : "deeper"),
          }));
        }
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

  // These screens manage selection and focus inside their own panels.
  let screenAct: ((a: Action) => boolean) | null = null;
  const ownInput = () => screen() === "games" || screen() === "social" || screen() === "sessions";

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
    if (screen() === "recovery" && recovery.confirming()) recovery.cancelReset();
    else if (screen() === "recovery" || screen() === "recover") go(s === "signup" ? "/dash/signup" : "/", "back");
    else if (screen() === "player" && s !== "signup") go(`/dash/${s}`, "back");
    else if (s === "server" && params.tag === "import" && !identity.me()) go("/dash/signin", "back");
    else if (s === "server" && params.tag) go("/dash/server", "back");
    else if (s === "signup" || s === "server") go("/dash/settings", "back");
    else if (s === "clans") go(`/dash/${menuHome("clans")}`, "back");
    else go("/", "back");
  }

  function select() {
    switch (screen()) {
      case "games": case "social": case "sessions": screenAct?.("select"); break;
      case "menu": {
        const id = items()[menu()]!;
        if (!lockedHint(id, !!identity.me())) go(`/dash/${id}`, "deeper");
        break;
      }
      case "recovery":
        if (recovery.confirming()) recovery.setupOrReset();
        else sats()[sat()]?.act?.();
        break;
      case "list": openRow(sel()); break;
      case "signup": (document.getElementById("dash-signup") as HTMLFormElement | null)?.requestSubmit(); break;
      case "recover": (document.getElementById("dash-recover") as HTMLFormElement | null)?.requestSubmit(); break;
      default: {
        const s = sats()[sat()];
        if (s?.act) s.act();
        else if (screen() === "server" && serverSub() === "connect") (document.getElementById("dash-server-form") as HTMLFormElement | null)?.requestSubmit();
      }
    }
  }

  /** One step of input, from the keyboard or a gamepad. Says whether it meant anything here. */
  function act(a: Action): boolean {
    if (leaving()) return false;
    if (ownInput()) return screenAct?.(a) ?? false;
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
        // A server plate's blades (invites, accounts, import) take the input first.
        if (screen() === "server" && blades()?.act(a)) return true;
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
    // Let a tab-focused link or prompt perform its own activation.
    if (e.key === "Enter" && (e.target as HTMLElement).closest?.("a[href], .dx-prompt")) return;
    const typing = (e.target as HTMLElement).closest?.("input, textarea, select");
    // A field in a column of rows (data-nav) still lets up and down move on.
    const rowField = typing && (e.target as HTMLElement).dataset?.nav && (e.key === "ArrowUp" || e.key === "ArrowDown");
    if (typing && e.key !== "Escape" && !rowField) return;
    const a = keyAction(e.key);
    if (a && act(a)) e.preventDefault();
  }
  // A gamepad steers the same way; in a form its stick leaves the text caret alone.
  function onPad(a: Action) {
    const typing = document.activeElement?.closest?.("input, textarea, select");
    const rowField = typing && (document.activeElement as HTMLElement).dataset?.nav && (a === "up" || a === "down");
    if (typing && a !== "select" && a !== "back" && !rowField) return;
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
      case "games": case "list": return { x: 0.66, y: 0.38, dim: 1 };
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
        <Show when={identity.me()} fallback={<Show when={section() !== "signin"}><a class="dx-chip dx-chip--link dx-chip--join" href="/dash/signin" onClick={(e) => { e.preventDefault(); go("/dash/signin", "deeper"); }}>sign in</a></Show>}>
          {(me) => <a class="dx-chip dx-chip--me" href="/dash/signup" onClick={(e) => { e.preventDefault(); if (screen() !== "player" || section() !== "signup") go("/dash/signup", section() === "signup" ? "back" : "deeper"); }}>{me().gamertag}</a>}
        </Show>
        <Show when={section() !== "games"}>
          <a class="dx-chip dx-chip--link" href="/dash/games">games</a>
        </Show>
      </header>

      <main class="dx-stage">
        <Switch>
          <Match when={screen() === "menu"}>
            <div class="dx-screen dx-menu" onWheel={onWheel}>
              <Orb turn={-turn() * RING_STEP_DEG} />
              <Show when={identity.me()}>{(me) => <div class="dx-orb-me">{me().gamertag}</div>}</Show>
              <div class="dx-orb-count" title={live.wellKnown()?.id ?? new URL(live.server().base).host}>
                <span class="dx-orb-server">{live.wellKnown()?.id ?? new URL(live.server().base).host}</span> <i /> <b>{liveGames(live.state).length}</b> live <i /> <b>{online().length}</b> online
              </div>
              <Show when={motdHeader(motd())}>{(h) => <p class="dx-motd">{h()}</p>}</Show>
              <nav class="dx-arc" aria-label="Dashboard" style={{ "--n": String(items().length) }}>
                <For each={items()}>
                  {(id, i) => {
                    const at = createMemo(() => place(i()));
                    return (
                      <button
                        class="dx-item"
                        classList={{ on: menu() === i(), hov: hover() === i(), "dx-item--far": at().opacity < 0.12, "dx-item--locked": !!lockedHint(id, !!identity.me()) }}
                        aria-disabled={lockedHint(id, !!identity.me()) ? "true" : undefined}
                        style={{ "--i": String(i()), "--x": at().x.toFixed(2), "--y": at().y.toFixed(2), "--s": at().scale.toFixed(4), "--o": at().opacity.toFixed(3), "--dr": at().drop.toFixed(2), "--lit": at().lit.toFixed(3) }}
                        onMouseEnter={() => hoverOn(i())}
                        onMouseLeave={hoverOff}
                        onFocus={() => setMenu(i())}
                        onClick={() => pickItem(i())}
                        aria-current={menu() === i() ? "true" : undefined}
                      >
                        <Vine />
                        <Pod seed={i()} />
                        <span class="dx-bar"><Bracket /><i class="dx-bar-lit" aria-hidden="true" /><span class="dx-bar-text">{LABELS[id]()}</span><Show when={lockedHint(id, !!identity.me())}>{(h) => <span class="dx-bar-hint">{h()}</span>}</Show></span>
                      </button>
                    );
                  }}
                </For>
              </nav>
              <Prompts onA={select} />
            </div>
          </Match>

          <Match when={screen() === "games"}>
            <GamesScreen back={back} bind={(f) => { screenAct = f; }} />
          </Match>

          <Match when={screen() === "list"}>
            <div class="dx-screen dx-list dx-pages">
              <ListPanel title={title()} rows={rows()} sel={sel()} onSel={setSel} onAct={openRow} empty={emptyText(section())} />
              <Show when={section() === "signin" || section() === "account"} fallback={<>
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
              </>}>
                <div class="dx-preview">
                  <BigPod>
                    <Show when={rows()[sel()]?.key.startsWith("#") === false ? rows()[sel()]!.key : null} keyed fallback={<Glyph name="key" />}>
                      {(xuid) => <Emblem seed={xuid} size={220} class="dx-emblem" />}
                    </Show>
                  </BigPod>
                </div>
                <SignInPlate />
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
              <SignUpPlate onRecovery={() => navigate("/dash/account")} />
              <Prompts onA={select} onB={back} aLabel="create" />
            </div>
          </Match>

          <Match when={screen() === "recovery"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages dx-recovery">
              <div class="dx-centre">
                <BigPod wire>
                  <Show when={recovery.phase() === "ready" && recovery.rec()} fallback={<Glyph name="key" />}><RecoveryCode state={recovery} /></Show>
                </BigPod>
              </div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <AccountPlate state={recovery} />
              <Prompts
                onA={select} onB={back}
                aLabel={recovery.confirming() ? "reset" : "select"}
                bLabel={recovery.confirming() ? "keep" : recovery.isNew() ? "done" : "back"}
              />
            </div>
          </Match>

          <Match when={screen() === "recover"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages dx-recover">
              <div class="dx-centre"><BigPod wire><Glyph name="key" /></BigPod></div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <RecoverPlate onDone={() => go("/dash/signup", "deeper", true)} />
              <Prompts onA={select} onB={back} aLabel="recover" />
            </div>
          </Match>

          <Match when={screen() === "social"}>
            <SocialScreen section={section() as "friends" | "clans"} tag={params.tag ? safeDecode(params.tag) : undefined} go={go} back={back} bind={(f) => { screenAct = f; }} />
          </Match>

          <Match when={screen() === "sessions"}>
            <SessionsScreen id={params.tag ? safeDecode(params.tag) : undefined} go={go} back={back} bind={(f) => { screenAct = f; }} />
          </Match>

          <Match when={screen() === "server" && serverSub() === "connect"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages">
              <div class="dx-centre"><BigPod wire><Glyph name="server" /></BigPod></div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <ConnectPlate onUse={useServer} busy={serverBusy()} err={serverErr()} />
              <Prompts onA={select} onB={back} aLabel="connect" />
            </div>
          </Match>

          <Match when={screen() === "server" && serverSub() === "invites"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages dx-server dx-invites-screen">
              <div class="dx-centre"><BigPod wire><Glyph name="key" /></BigPod></div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <InvitesPlate info={serverInfo} bind={bindPlate("invites")} />
              <Prompts onA={() => act("select")} onB={back} onX={blades()?.alt() ? () => act("alt") : undefined} xLabel={blades()?.alt() ?? undefined} />
            </div>
          </Match>

          <Match when={screen() === "server" && serverSub() === "accounts"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages dx-server dx-invites-screen">
              <div class="dx-centre"><BigPod wire><Glyph name="career" /></BigPod></div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <AccountsPlate info={serverInfo} go={(href) => go(href, "deeper")} bind={bindPlate("accounts")} />
              <Prompts onA={() => act("select")} onB={back} />
            </div>
          </Match>

          <Match when={screen() === "server" && serverSub() === "reports"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages dx-server dx-reports-screen">
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <Plate small="your server" title="player reports" main>
                <ReportsQueue />
              </Plate>
              <Prompts onA={select} onB={back} />
            </div>
          </Match>

          <Match when={screen() === "server" && serverSub() === "bans"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages dx-server dx-reports-screen">
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <Plate small="your server" title="bans" main>
                <BansList />
              </Plate>
              <Prompts onA={select} onB={back} />
            </div>
          </Match>

          <Match when={screen() === "server" && serverSub() === "import"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages dx-server dx-invites-screen dx-import-screen">
              <div class="dx-centre"><BigPod wire><Glyph name="download" /></BigPod></div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <ImportPlate onDone={() => go("/dash/account", "deeper")} bind={bindPlate("import")} />
              <Prompts onA={() => act("select")} onB={back} />
            </div>
          </Match>

          <Match when={screen() === "server"}>
            <div class="dx-screen dx-pods dx-pods--form dx-pages dx-server">
              <div class="dx-centre"><BigPod wire><Glyph name="globe" /></BigPod></div>
              <Sats sats={sats()} sel={sat()} onSel={setSat} />
              <ServerPlate info={serverInfo} />
              <Prompts onA={select} onB={back} />
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
    case "players": return "nobody here yet";
    case "history": return "no finished games yet";
    default: return "nothing here yet";
  }
}

export function ListPanel(props: { title: string; rows: Row[]; sel: number; onSel: (i: number) => void; onAct: (i: number) => void; empty: string; hint?: string }) {
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
      <div class="dx-list-foot"><span>h2 live</span><span>{props.hint ?? "↑ ↓ browse"}</span></div>
    </section>
  );
}

const NO_FRAME: MapFrame = { players: [], markers: [], seen: null };

/** The orb's picture of a game: the map's picture (preview, else its top
 *  view) under the orb's glass, with a live game's blips over it; without
 *  one, the drawn outline as before. */
function GameMap(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  const picture = mapPicture(() => props.game.map, () => props.game.map_id);
  const frame = props.game.status === "live" ? liveFrameSource(props.game.game_id, roster) : () => NO_FRAME;
  return (
    <div class="dx-map" classList={{ "dx-map--pic": !!picture() }}>
      <Show when={picture()}>{(u) => <img class="dx-map-pic" src={u()} alt={`${gameMapTitle(props.game)}`} decoding="async" draggable={false} />}</Show>
      <Show when={!picture() || props.game.status === "live"}>
        <MapCanvas mini frame={frame} roster={roster} outline={() => (picture() ? null : outline())} label={`map of ${gameMapTitle(props.game)}`} />
      </Show>
    </div>
  );
}

function GamePlate(props: { game: GameState }) {
  const g = () => props.game;
  return (
    <Plate small={g().status === "live" ? "live" : "final"} title={gameMapTitle(g())}>
      <dl>
        <dt>game</dt><dd>{gameTypeLine(g())}</dd>
        <Show
          when={isCampaign(g())}
          fallback={<>
            <dt>players</dt><dd>{Object.keys(g().players).length}</dd>
            <dt>{g().status === "live" ? "leading" : "final"}</dt><dd>{gameLine(g())}</dd>
          </>}
        >
          <dt>players</dt><dd>{Object.values(g().players).map((p) => p.name).join(", ")}</dd>
          <dt>result</dt><dd>{OUTCOME_WORDS[campaignOutcome(g())]}</dd>
        </Show>
        <dt>{g().status === "live" ? "clock" : "ended"}</dt><dd>{g().status === "live" ? clock(elapsedSeconds(g(), now())) : ago(g().ended_at, now())}</dd>
      </dl>
    </Plate>
  );
}

const PRESENCE_WORD = { playing: "in a game", online: "online", away: "away", offline: "offline" } as const;

function PlayerPlate(props: { account: AccountState; main?: boolean; mine?: boolean }) {
  // Games, wins and K/D are multiplayer's; campaign is a line of its own.
  const rec = createMemo(() => serviceRecord(live.state, props.account.xuid));
  const camp = createMemo(() => campaignRecord(live.state, props.account.xuid, now()));
  return (
    <Plate small={props.mine ? "signed in" : PRESENCE_WORD[presenceKind(props.account)]} title={props.account.gamertag} main={props.main}>
      <dl>
        <dt>status</dt><dd>{presenceText(props.account)}</dd>
        <dt>games</dt><dd>{rec().games} <span>played</span> {rec().wins} <span>won</span></dd>
        <dt>kills / deaths</dt><dd>{rec().kills} / {rec().deaths}</dd>
        <Show when={camp().games}><dt>campaign</dt><dd>{camp().games} <span>played</span> {camp().completed} <span>complete</span></dd></Show>
        <dt>rank</dt><dd>{rec().rank}</dd>
        <Show when={props.mine}><dt>key</dt><dd>kept in this browser</dd></Show>
      </dl>
      <Show when={props.mine && props.main}><BanNotice /><MutedList /></Show>
      <Show when={!props.mine && props.main && identity.me()}>
        <MuteButton target={{ xuid: props.account.xuid, gamertag: props.account.gamertag }} />
      </Show>
      <Show when={!props.mine && props.main && adminState.isMeAdmin() === true}>
        <BanControl target={{ xuid: props.account.xuid, gamertag: props.account.gamertag }} />
      </Show>
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

/** The sign-in list's plate: what signing in here means. */
function SignInPlate() {
  return (
    <Plate small="h2 live" title="sign in">
      <dl>
        <dt>server</dt><dd>{live.wellKnown()?.id ?? new URL(live.server().base).host}</dd>
        <dt>kept here</dt><dd>{identity.accounts().length} <span>{identity.accounts().length === 1 ? "account" : "accounts"}</span></dd>
      </dl>
      <p class="dx-note">New device? Recover your account with its gamertag and a code. Coming from the game? Open the sign-in link it shows.</p>
    </Plate>
  );
}

function SignUpPlate(props: { onRecovery: () => void }) {
  const [tag, setTag] = createSignal("");
  const [invite, setInvite] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  // The invite field only where the server asks for one (GET /v1/server);
  // an older server without it gets the field, to be safe.
  const [inviteOnly, setInviteOnly] = createSignal<boolean | null>(null);
  createEffect(on(() => live.server().api, (api) => {
    setInviteOnly(null);
    fetchServerFacts(api).then((f) => setInviteOnly(f ? f.invite_only : true), () => setInviteOnly(true));
  }));
  const [err, setErr] = createSignal<string | null>(null);
  async function submit(e: Event) {
    e.preventDefault();
    setErr(null);
    const bad = gamertagError(tag().trim());
    if (bad) { setErr(`gamertag: ${bad}`); return; }
    setBusy(true);
    try {
      const wk = live.wellKnown() ?? (await fetchWellKnown(live.server().base));
      const { account, recovery } = await signUp(live.server().api, live.server().base, wk.id, tag().trim(), (inviteOnly() !== false && invite().trim()) || undefined);
      // The recovery setup shows once, on the next screen (an older server sends none).
      if (recovery) { keepFreshRecovery(account.xuid, recovery); props.onRecovery(); }
      await identity.add(account);
    } catch (x) {
      setErr(x instanceof SignUpError ? x.message : `couldn't sign up: ${(x as Error).message}`);
    } finally {
      setBusy(false);
    }
  }
  onMount(() => { if (matchMedia("(pointer: fine)").matches) document.getElementById("dash-gamertag")?.focus({ preventScroll: true }); });
  return (
    <Plate small="new account" title="create account" main>
      <p class="dx-signup-intro">Choose your name. Join the game.</p>
      <form id="dash-signup" class="dx-form" onSubmit={submit}>
        <label for="dash-gamertag">gamertag</label>
        <input id="dash-gamertag" value={tag()} maxLength={15} onInput={(e) => setTag(e.currentTarget.value)} autocomplete="off" spellcheck={false} placeholder="your name, up to 15 characters" aria-describedby="dash-signup-note" />
        <Show when={inviteOnly() === true}>
          <label for="dash-invite">invite code</label>
          <input id="dash-invite" value={invite()} onInput={(e) => setInvite(e.currentTarget.value)} autocomplete="off" spellcheck={false} placeholder="from a friend on this server" />
        </Show>
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
