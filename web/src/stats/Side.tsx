// Pieces the stats pages share in their menu column: the pill menu, the
// People Finder, the Recent Games carousel, and map thumbnails.

import GametypeIcon from "../components/GametypeIcon";
import { mapKey } from "../lib/gameart";
import { gameImage } from "../state/gameart";
import { For, Show, createMemo, createSignal, type JSX } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import { live } from "../state/live";
import { standings, teamStandings, type GameState } from "../state/reducer";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import { colourFor, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { ago, titleCase } from "../lib/format";
import { now } from "../lib/now";
import { outlineFor } from "../state/outlines";
import { Panel } from "./Layout";

export interface PillDef { label: JSX.Element; href: string; end?: boolean }

export function Pills(props: { items: PillDef[]; label: string }) {
  return (
    <nav class="s-pills" aria-label={props.label}>
      <For each={props.items}>
        {(p) => (p.href.startsWith("#") ? <a href={p.href}>{p.label}</a> : <A href={p.href} end={p.end}>{p.label}</A>)}
      </For>
    </nav>
  );
}

export function PeopleFinder() {
  const navigate = useNavigate();
  const [who, setWho] = createSignal("");
  const [where, setWhere] = createSignal("record");
  function go(e: Event) {
    e.preventDefault();
    const t = who().trim();
    if (!t) return;
    const a = Object.values(live.state.accounts).find((x) => x.gamertag.toLowerCase() === t.toLowerCase());
    if (!a) { navigate(`/stats/players?q=${encodeURIComponent(t)}`); return; }
    const base = `/stats/players/${encodeURIComponent(a.gamertag)}`;
    navigate(where() === "record" ? base : `${base}/${where()}`);
  }
  return (
    <Panel title="People Finder" sub="find by gamertag on this server" class="s-finder-box">
      <form class="s-finder" onSubmit={go}>
        <label class="sr-only" for="s-finder-where">Open</label>
        <select id="s-finder-where" value={where()} onChange={(e) => setWhere(e.currentTarget.value)}>
          <option value="record">Service Record</option>
          <option value="history">Game History</option>
          <option value="career">Career Stats</option>
        </select>
        <div class="s-finder-row">
          <label class="sr-only" for="s-finder">Gamertag</label>
          <input id="s-finder" value={who()} onInput={(e) => setWho(e.currentTarget.value)} list="s-finder-list" />
          <datalist id="s-finder-list"><For each={Object.values(live.state.accounts)}>{(x) => <option value={x.gamertag} />}</For></datalist>
          <button type="submit" class="s-btn">search</button>
        </div>
      </form>
    </Panel>
  );
}

/** Where the knockouts of a finished game happened: a still for its thumbnail. */
function stillFrame(g: GameState): (t: number) => MapFrame {
  let cached: MapFrame | null = null;
  let n = -1;
  return () => {
    if (cached && n === g.kills.length) return cached;
    n = g.kills.length;
    const seen = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity };
    const markers = [];
    for (const k of g.kills) {
      if (!k.pos) continue;
      const [x, y, z] = k.pos;
      seen.minX = Math.min(seen.minX, x); seen.maxX = Math.max(seen.maxX, x);
      seen.minY = Math.min(seen.minY, y); seen.maxY = Math.max(seen.maxY, y);
      const v = g.players[k.victim];
      markers.push({ x, y, z, alpha: 0.9, colour: v ? colourFor(g, v.team, v.index) : "#888" });
    }
    cached = { players: [], markers, seen: markers.length ? seen : null };
    return cached;
  };
}

/** A small picture of a game. With the server's game art it is the map's
 *  preview (a live game's blips drawn over it); otherwise a top-down view:
 *  live blips, or where a finished game's knockouts fell. */
export function MapThumb(props: { game: GameState; class?: string; large?: boolean }) {
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  const liveSrc = liveFrameSource(props.game.game_id, roster);
  const still = stillFrame(props.game);
  const frame = (t: number) => (props.game.status === "live" ? liveSrc(t) : still(t));
  const preview = () => gameImage(mapKey(props.game.map));
  return (
    <span class={`s-thumb ${props.class ?? ""}`} classList={{ "s-thumb--live": props.game.status === "live", "s-thumb--pic": !!preview() }}>
      <Show when={preview()}>{(p) => <img class="s-thumb-pic" src={p().url} alt={`${titleCase(props.game.map)} preview`} decoding="async" draggable={false} />}</Show>
      <Show when={!preview() || props.game.status === "live"}>
        <MapCanvas mini={!props.large} frame={frame} roster={roster} outline={outline} still={props.game.status !== "live"} label={`Map of ${titleCase(props.game.map)}`} />
      </Show>
      <Show when={props.game.status === "live"}><span class="s-live-tag">LIVE</span></Show>
    </span>
  );
}

/** A map's preview picture, small, for a table row. Nothing without game art. */
export function MapPreview(props: { map: string }) {
  return <Show when={gameImage(mapKey(props.map))}>{(p) => <img class="s-mappic" src={p().url} alt="" decoding="async" draggable={false} />}</Show>;
}

/** The gamertag of whoever started the game (the server names them `acct:<xuid>@<server>`). */
export function hostName(g: GameState): string | null {
  const xuid = g.host?.match(/^acct:([^@]+)@/)?.[1] ?? g.console;
  const tag = xuid ? live.state.accounts[xuid]?.gamertag : undefined;
  return tag ?? Object.values(g.players).find((p) => p.local)?.name ?? null;
}

export function gameTitle(g: GameState): string {
  return titleCase(g.variant ?? g.mode);
}

export function playlist(g: GameState): string {
  return g.team_play ? "Team Game" : "Free for All";
}

/** Who is ahead (or won), in a few words. */
export function leader(g: GameState): string {
  if (g.team_play) {
    const t = teamStandings(g)[0];
    return t ? `${teamName(t.team)} Team` : "–";
  }
  return standings(g)[0]?.name ?? "–";
}

/** The Recent Games box: one game at a time, stepped with the arrows. */
export function RecentGames(props: { games: GameState[]; title?: string; empty?: string }) {
  const [at, setAt] = createSignal(0);
  const n = () => props.games.length;
  const g = () => props.games[Math.min(at(), Math.max(0, n() - 1))];
  const step = (d: number) => setAt((Math.min(at(), Math.max(0, n() - 1)) + d + n()) % Math.max(1, n()));
  return (
    <section class="s-box s-recent">
      <header class="s-box-head s-recent-head">
        <button class="s-arrow" onClick={() => step(-1)} disabled={n() < 2} aria-label="Previous game">◄</button>
        <h3>{props.title ?? "Recent Games"}</h3>
        <button class="s-arrow" onClick={() => step(1)} disabled={n() < 2} aria-label="Next game">►</button>
      </header>
      <div class="s-box-body">
        <Show when={g()} keyed fallback={<p class="s-recent-empty">{props.empty ?? "No games on record yet."}</p>}>
          {(game) => (
            <A href={`/stats/games/${game.game_id}`} class="s-recent-item">
              <MapThumb game={game} />
              <ul>
                <li class="s-recent-title"><GametypeIcon variant={game.variant} />{gameTitle(game)}</li>
                <li>On {titleCase(game.map)}</li>
                <li>Playlist: {playlist(game)}</li>
                <li>{game.status === "live" ? "In progress now" : `Finished ${ago(game.ended_at, now())}`}</li>
                <li>{game.status === "live" ? "Leader" : "MVP"}: <b>{standings(game)[0]?.name ?? "–"}</b></li>
              </ul>
            </A>
          )}
        </Show>
        <div class="s-recent-well">{n() ? `${Math.min(at(), n() - 1) + 1} of ${n()}` : ""}</div>
      </div>
    </section>
  );
}
