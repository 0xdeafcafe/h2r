// Pieces the stats pages share in their menu column: the pill menu, the
// People Finder, the Recent Games carousel, the Did You Know? box, and map
// thumbnails.

import GametypeIcon from "../components/GametypeIcon";
import { SkullBadges } from "../components/SkullIcon";
import { gameMapTitle, packageLookup } from "../state/maps";
import { isHalo1Map } from "../state/gameart";
import { geometryChoice } from "../lib/maps";
import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, type JSX } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import { live } from "../state/live";
import { isCampaign, standings, teamStandings, type GameState } from "../state/reducer";
import { campaignLabel, difficultyName, gameKind, gameTypeLabel, variantLabel, variantName } from "../state/gametype";
import { OUTCOME_WORDS, campaignOutcome } from "../state/derive";
import MapPicture, { mapPicture } from "../components/MapPicture";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import { colourFor, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { ago, titleCase } from "../lib/format";
import { now } from "../lib/now";
import { outlineFor } from "../state/outlines";
import { geometryFor } from "../map/geometry";
import ViewMap from "../map/ViewMap";
import { topView } from "../map/views";
import { Panel } from "./Layout";
import { randomStart, stepIndex } from "../lib/motd";
import { STATS_ROOT } from "../routes";

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
    if (!a) { navigate(`${STATS_ROOT}/players?q=${encodeURIComponent(t)}`); return; }
    const base = `${STATS_ROOT}/players/${encodeURIComponent(a.gamertag)}`;
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
          <option value="campaign">Campaign Record</option>
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
 *  preview (a live game's blips drawn over it); otherwise a top-down view
 *  on the map's outline: live blips, or where a finished game's knockouts
 *  fell; with neither, a card with the map's name. With `top` (the game
 *  page's header) a finished game shows the map's preview too; a live one,
 *  or a map with no preview, the pre-drawn top view when the server has
 *  one, with the same blips or knockouts plotted on it. */
export function MapThumb(props: { game: GameState; class?: string; large?: boolean; top?: boolean }) {
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  const liveSrc = liveFrameSource(props.game.game_id, roster);
  const still = stillFrame(props.game);
  const frame = (t: number) => (props.game.status === "live" ? liveSrc(t) : still(t));
  // The package's image, else the map's preview, else its top view, else a
  // drawn outline, else (a finished game) a plain card with the map's name.
  const picture = mapPicture(() => props.game.map, () => props.game.map_id);
  // A picture that fails to load (art moved, a stale publish) falls back
  // like no picture at all, never the browser's broken-image mark.
  const [brokenPic, setBrokenPic] = createSignal<string | null>(null);
  const preview = () => { const p = picture(); return p && p !== brokenPic() ? p : null; };
  // Only a `top` thumbnail asks for the map's views (a views.json per map),
  // and not for a mod whose geometry isn't the base map's.
  const sameGeometry = () => geometryChoice(packageLookup(props.game.map_id)) === "base";
  // The header of a finished game shows the map's own picture (as the
  // game's Select Map screen does) when there is one; the top view is for
  // a live game's blips, or a map with no picture.
  const wantTop = () => sameGeometry() && (props.game.status === "live" || !preview());
  const geometry = props.top ? geometryFor(() => (wantTop() ? props.game.map : undefined)) : () => null;
  const [broken, setBroken] = createSignal(false);
  createEffect(on(() => props.game.map, () => setBroken(false), { defer: true }));
  const top = () => (broken() ? null : topView(geometry()?.views));
  return (
    <Show
      when={top()}
      fallback={
        <span class={`s-thumb ${props.class ?? ""}`} classList={{ "s-thumb--live": props.game.status === "live", "s-thumb--pic": !!preview() }}>
          <Show when={preview()}>{(p) => <img class="s-thumb-pic" src={p()} alt={`${gameMapTitle(props.game)} preview`} decoding="async" draggable={false} onError={() => setBrokenPic(p())} />}</Show>
          <Show when={!preview() && props.game.status !== "live" && !outline()}><MapPlaceholder game={props.game} /></Show>
          <Show when={props.game.status === "live" || (!preview() && outline())}>
            <MapCanvas mini={!props.large} frame={frame} roster={roster} outline={outline} still={props.game.status !== "live"} label={`Map of ${gameMapTitle(props.game)}`} />
          </Show>
          <Show when={props.game.status === "live"}><span class="s-live-tag">LIVE</span></Show>
        </span>
      }
    >
      {(v) => (
        <span class={`s-thumb s-thumb--top ${props.class ?? ""}`} classList={{ "s-thumb--live": props.game.status === "live" }}>
          <ViewMap mini frame={frame} roster={roster} views={geometry()!.views!} view={v().id} image={geometry()!.image} label={`Map of ${gameMapTitle(props.game)}`} onError={() => setBroken(true)} />
          <Show when={props.game.status === "live"}><span class="s-live-tag">LIVE</span></Show>
        </span>
      )}
    </Show>
  );
}

/** What a game's thumbnail shows with no picture of the map and no outline
 *  to draw on: the map's name on a plain card, rather than an empty grid. */
export function MapPlaceholder(props: { game: GameState }) {
  return (
    <span class="s-thumb-none" aria-hidden="true">
      <GametypeIcon variant={isCampaign(props.game) ? null : props.game.variant} size={18} />
      <b>{gameMapTitle(props.game)}</b>
      <small>{isHalo1Map(props.game.map) ? "Halo 1" : isCampaign(props.game) ? "Campaign" : variantName(props.game.variant)}</small>
    </span>
  );
}

/** A map's picture, small, for a table row. Nothing without game art. */
export function MapPreview(props: { map: string; mapId?: string | null }) {
  return <MapPicture map={props.map} mapId={props.mapId} class="s-mappic" />;
}

/** The gamertag of whoever started the game (the server names them `acct:<xuid>@<server>`). */
export function hostName(g: GameState): string | null {
  const xuid = g.host?.match(/^acct:([^@]+)@/)?.[1] ?? g.console;
  const tag = xuid ? live.state.accounts[xuid]?.gamertag : undefined;
  return tag ?? Object.values(g.players).find((p) => p.local)?.name ?? null;
}

/** A game's headline: the variant for multiplayer, the campaign label
 *  ("Campaign Co-op") for campaign. */
export function gameTitle(g: GameState): string {
  return isCampaign(g) ? campaignLabel(g) : g.variant ? variantLabel(g.variant) : titleCase(g.mode);
}

/** The game's type for a "Playlist:" line: "Matchmaking: <playlist>",
 *  "Custom Game, Team Game", or the campaign label with its difficulty. */
export function playlist(g: GameState): string {
  if (gameKind(g) === "campaign") return [campaignLabel(g), difficultyName(g.difficulty)].filter(Boolean).join(", ");
  const label = gameTypeLabel(g);
  return gameKind(g) === "matchmaking" ? label : `${label}, ${g.team_play ? "Team Game" : "Free for All"}`;
}

/** A campaign game's players, by name. */
export function fireteam(g: GameState): string {
  return Object.values(g.players).map((p) => p.name).join(", ") || "–";
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
            <A href={`${STATS_ROOT}/games/${game.game_id}`} class="s-recent-item">
              <MapThumb game={game} />
              <ul>
                <li class="s-recent-title"><GametypeIcon variant={game.variant} />{gameTitle(game)}</li>
                <li>On {gameMapTitle(game)}</li>
                <li>{isCampaign(game) ? difficultyName(game.difficulty) ?? "Campaign" : `Playlist: ${playlist(game)}`}</li>
                <Show when={game.skulls.length}><li><SkullBadges skulls={game.skulls} size={16} /></li></Show>
                <li>{game.status === "live" ? "In progress now" : `${isCampaign(game) ? OUTCOME_WORDS[campaignOutcome(game)] : "Finished"} ${ago(game.ended_at, now())}`}</li>
                <Show when={isCampaign(game)} fallback={<li>{game.status === "live" ? "Leader" : "MVP"}: <b>{standings(game)[0]?.name ?? "–"}</b></li>}>
                  <li>Players: <b>{fireteam(game)}</b></li>
                </Show>
              </ul>
            </A>
          )}
        </Show>
        <div class="s-recent-well">{n() ? `${Math.min(at(), n() - 1) + 1} of ${n()}` : ""}</div>
      </div>
    </section>
  );
}

/** The Did You Know? box: the server's message-of-the-day messages, one at
 *  a time from a random start, moving on every `everyMs` (not under
 *  reduced motion, nor while hovered or focused) and stepped with the
 *  arrows. Nothing when there are no messages. */
export function DidYouKnow(props: { messages: string[]; everyMs?: number }) {
  const n = () => props.messages.length;
  const [at, setAt] = createSignal(0);
  const [held, setHeld] = createSignal(false);
  const [tick, setTick] = createSignal(0);
  createEffect(on(n, (len) => setAt(randomStart(len))));
  const i = () => stepIndex(at(), 0, n());
  const step = (d: number) => { setAt(stepIndex(at(), d, n())); setTick(tick() + 1); };
  const still = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  createEffect(on([n, held, tick], ([len, h]) => {
    if (still || h || len < 2) return;
    const t = setInterval(() => setAt((a) => stepIndex(a, 1, n())), props.everyMs ?? 12_000);
    onCleanup(() => clearInterval(t));
  }));
  return (
    <Show when={n() > 0}>
      <section
        class="s-box s-recent s-dyk"
        onMouseEnter={() => setHeld(true)}
        onMouseLeave={() => setHeld(false)}
        onFocusIn={() => setHeld(true)}
        onFocusOut={() => setHeld(false)}
      >
        <header class="s-box-head s-recent-head">
          <button class="s-arrow" onClick={() => step(-1)} disabled={n() < 2} aria-label="Previous tip">◄</button>
          <h3>Did You Know?</h3>
          <button class="s-arrow" onClick={() => step(1)} disabled={n() < 2} aria-label="Next tip">►</button>
        </header>
        <div class="s-box-body">
          <Show when={props.messages[i()]} keyed>{(m) => <p class="s-dyk-text">{m}</p>}</Show>
          <div class="s-recent-well">{n() > 1 ? `${i() + 1} of ${n()}` : ""}</div>
        </div>
      </section>
    </Show>
  );
}
