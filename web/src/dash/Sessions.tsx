// "Find a game": the open sessions hosts are advertising, as a dashboard
// list screen (host, map and players against the most; a preview pod; a
// plate with the selected session's details), and one session on a pod
// screen of its own with its roster. Both refresh every few seconds. There
// is nothing to join from here: joining happens in the game.

import { For, Match, Show, Switch, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { gameImage, mapTitle } from "../state/gameart";
import { mapKey } from "../lib/gameart";
import { live } from "../state/live";
import { now } from "../lib/now";
import { gametype, keepSelection, kind, listed, mapName, openFor, roomText, seats, sessionRow, sortSessions, visibilityText, type Session } from "../lib/sessions";
import { useSession, useSessions } from "../state/sessions";
import Emblem from "../components/Emblem";
import { BigPod, Glyph, Plate, Prompts } from "./parts";
import type { Action } from "./input";
import { ListPanel } from "./Dashboard";
import "../styles/dash-sessions.css";

export interface SessionsProps {
  /** A session id from the route: that session's own screen. */
  id?: string;
  go: (href: string, dir: "deeper" | "back") => void;
  back: () => void;
  /** Hands the screen's input handler to the dashboard. */
  bind: (fn: ((a: Action) => boolean) | null) => void;
}

export default function SessionsScreen(props: SessionsProps) {
  return (
    <Show when={props.id} keyed fallback={<SessionList {...props} />}>
      {(id) => <SessionDetail {...props} id={id} />}
    </Show>
  );
}

/** The map's picture from the server's game art, else the host's emblem. */
function SessionPod(props: { session: Session | null | undefined }) {
  const pic = () => (props.session?.map ? gameImage(mapKey(props.session.map)) : null);
  return (
    <Show when={props.session} fallback={<Glyph name="globe" />}>
      {(s) => (
        <Show when={pic()} fallback={<Emblem seed={s().host_xuid || s().host_gamertag} size={220} class="dx-emblem" />}>
          {(p) => <img class="dx-session-pic" src={p().url} alt="" decoding="async" draggable={false} />}
        </Show>
      )}
    </Show>
  );
}

/** Seats as a row of pips, filled ones lit. */
function Seats(props: { session: Session }) {
  const n = () => seats(props.session);
  return (
    <span class="dx-seats" role="img" aria-label={`${n().filled} of ${n().max} players`}>
      <For each={Array.from({ length: n().max }, (_, i) => i < n().filled)}>{(on) => <i classList={{ on }} />}</For>
    </span>
  );
}

function Facts(props: { session: Session }) {
  const s = () => props.session;
  return (
    <dl>
      <dt>map</dt><dd>{mapName(s(), mapTitle)}</dd>
      <dt>game</dt><dd>{gametype(s())}</dd>
      <dt>players</dt><dd>{seats(s()).filled} <span>of</span> {seats(s()).max} <span>{roomText(s())}</span></dd>
      <dt>type</dt><dd>{kind(s())}</dd>
      <dt>open to</dt><dd>{visibilityText(s())}</dd>
      <dt>open for</dt><dd>{openFor(s().since, now())}</dd>
    </dl>
  );
}

function SessionList(props: SessionsProps) {
  const feed = useSessions();
  const list = createMemo(() => sortSessions(feed.items().filter(listed)));
  const rows = createMemo(() => list().map((s) => sessionRow(s, mapTitle)));
  const [sel, setSel] = createSignal(0);
  // The list reorders as it refreshes: stay on the session, not the row number.
  let key: string | null = null;
  const choose = (i: number) => { setSel(i); key = rows()[i]?.key ?? null; };
  createEffect(() => { const i = keepSelection(rows(), key, sel()); setSel(i); key = rows()[i]?.key ?? null; });
  const current = () => list()[sel()];
  const open = (i = sel()) => { const r = rows()[i]; if (r) props.go(`/dash/sessions/${r.key}`, "deeper"); };

  function act(a: Action): boolean {
    const n = rows().length;
    switch (a) {
      case "down": choose(Math.min(n - 1, sel() + 1)); return true;
      case "up": choose(Math.max(0, sel() - 1)); return true;
      case "pageDown": choose(Math.min(n - 1, sel() + 6)); return true;
      case "pageUp": choose(Math.max(0, sel() - 6)); return true;
      case "select": case "right": open(); return true;
      case "back": case "left": props.back(); return true;
      default: return false;
    }
  }
  onMount(() => props.bind(act));
  onCleanup(() => props.bind(null));

  const empty = () => {
    switch (feed.status()) {
      case "loading": return "looking for games";
      case "unsupported": return "this server doesn't list sessions";
      case "error": return "can't reach the server";
      default: return "no open games right now";
    }
  };
  return (
    <div class="dx-screen dx-list dx-pages dx-sessions">
      <ListPanel title="find a game" rows={rows()} sel={sel()} onSel={choose} onAct={(i) => { choose(i); open(i); }} empty={empty()} />
      <div class="dx-preview"><BigPod><SessionPod session={current()} /></BigPod></div>
      <Show
        when={current()}
        fallback={
          <Plate small={feed.status() === "loading" ? "searching" : "sessions"} title={feed.status() === "loading" ? "looking" : "nothing open"}>
            <p class="dx-session-note">
              <Switch fallback="Nobody is hosting right now. Start a game on h2 live and it shows up here for others to find.">
                <Match when={feed.status() === "loading"}>Asking the server for open games.</Match>
                <Match when={feed.status() === "unsupported"}>This server doesn't keep a list of open games.</Match>
                <Match when={feed.status() === "error"}>The server didn't answer. Trying again in a few seconds.</Match>
              </Switch>
            </p>
          </Plate>
        }
      >
        {(s) => (
          <Plate small={feed.status() === "error" ? "stale" : roomText(s())} title={s().host_gamertag}>
            <Facts session={s()} />
            <Seats session={s()} />
          </Plate>
        )}
      </Show>
      <Prompts onA={rows().length ? () => open() : undefined} onB={props.back} aLabel="details" />
    </div>
  );
}

function SessionDetail(props: SessionsProps & { id: string }) {
  const one = useSession(() => props.id);
  const toList = () => props.go("/dash/sessions", "back");
  const host = () => { const s = one.session(); if (s) props.go(`/dash/players/${encodeURIComponent(s.host_gamertag)}`, "deeper"); };
  onMount(() => props.bind((a) => {
    if (a === "back" || a === "left") { toList(); return true; }
    if (a === "select") { host(); return true; }
    return false;
  }));
  onCleanup(() => props.bind(null));
  /** The roster by gamertag, host first; an XUID the web doesn't know shows as a guest. */
  const roster = createMemo(() => {
    const s = one.session();
    if (!s) return [];
    return Object.keys(s.players)
      .map((xuid) => ({ xuid, tag: xuid === s.host_xuid ? s.host_gamertag : live.state.accounts[xuid]?.gamertag ?? "guest", host: xuid === s.host_xuid }))
      .sort((a, b) => Number(b.host) - Number(a.host) || a.tag.localeCompare(b.tag));
  });
  return (
    <div class="dx-screen dx-pods dx-pages dx-session">
      <div class="dx-centre"><BigPod wire><SessionPod session={one.session()} /></BigPod></div>
      <Show
        when={one.session()}
        fallback={
          <Plate small="session" title={one.status() === "loading" ? "loading" : one.status() === "error" ? "no answer" : "not found"} main>
            <p class="dx-session-note">{one.status() === "loading" ? "Asking the server about this game." : one.status() === "error" ? "The server didn't answer. Trying again in a few seconds." : "This game isn't on the server any more."}</p>
          </Plate>
        }
      >
        {(s) => (
          <Plate small={one.status() === "error" ? "stale" : roomText(s())} title={s().host_gamertag} main>
            <Facts session={s()} />
            <Seats session={s()} />
            <Show when={roster().length}>
              <ul class="dx-roster" aria-label="Players in this game">
                <For each={roster()}>{(p) => <li classList={{ host: p.host }}>{p.tag}<Show when={p.host}><small>host</small></Show></li>}</For>
              </ul>
            </Show>
            <p class="dx-session-note">{s().state === "open" ? "Join from the game: h2 live, find a game." : "This game is no longer open."}</p>
          </Plate>
        )}
      </Show>
      <Prompts onA={one.session() ? host : undefined} onB={toList} aLabel="host" />
    </div>
  );
}
