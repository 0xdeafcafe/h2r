// The stats home's "Live Now" panel when nothing is live: the game that
// finished last, looped as a short top-down replay, and a few highlights
// from the games of the last day. Pure, so tests share them.

import { bestSprees } from "../state/derive";
import { isCampaign, type GameState } from "../state/reducer";

/** Game time looped at most: the closing stretch of the game. */
export const LOOP_GAME_SECONDS = 90;
/** About how long one pass of the loop takes on the wall clock. */
export const LOOP_WALL_SECONDS = 30;
/** Held on the final moment before the loop starts over. */
export const LOOP_HOLD_MS = 1500;

export interface LoopSpan { from: number; to: number; speed: number }

/** The stretch of a recording to loop (ticks), and how fast: the last
 *  `LOOP_GAME_SECONDS` of it, sped up so a pass takes about
 *  `LOOP_WALL_SECONDS` (never slower than real time, never above 4x). */
export function loopSpan(startTick: number, endTick: number, ticksPerSecond: number): LoopSpan | null {
  if (!(endTick > startTick)) return null;
  const from = Math.max(startTick, endTick - LOOP_GAME_SECONDS * ticksPerSecond);
  const seconds = (endTick - from) / ticksPerSecond;
  const speed = Math.min(4, Math.max(1, seconds / LOOP_WALL_SECONDS));
  return { from, to: endTick, speed };
}

/** A loop over a span that can be paused (off-screen, a hidden tab) and
 *  picks up where it left off. Wall time in ms, as `performance.now()`. */
export class LoopClock {
  private elapsed = 0;
  private since: number | null = null;

  constructor(readonly span: LoopSpan, readonly ticksPerSecond: number) {}

  get running(): boolean { return this.since !== null; }

  play(now: number): void { if (this.since === null) this.since = now; }

  pause(now: number): void {
    if (this.since === null) return;
    this.elapsed += now - this.since;
    this.since = null;
  }

  /** The tick shown at `now`: through the span, a short hold on its last
   *  tick, then from the top again. */
  tickAt(now: number): number {
    const { from, to, speed } = this.span;
    const ms = this.elapsed + (this.since === null ? 0 : now - this.since);
    const passMs = ((to - from) / this.ticksPerSecond / speed) * 1000;
    const t = ms % (passMs + LOOP_HOLD_MS);
    return t >= passMs ? to : from + (t / 1000) * this.ticksPerSecond * speed;
  }
}

/** The game the panel shows: the newest finished one, preferring
 *  multiplayer (a campaign run's last minute is mostly AI) when one ended
 *  in the same hour. `games` newest first, as `endedGames` gives them. */
export function pickLastGame(games: readonly GameState[]): GameState | null {
  const first = games.find((g) => g.status === "ended") ?? null;
  if (!first || !isCampaign(first)) return first;
  const endOf = (g: GameState) => Date.parse(g.ended_at ?? g.started_at);
  const mp = games.find((g) => g.status === "ended" && !isCampaign(g));
  return mp && endOf(first) - endOf(mp) < 3_600_000 ? mp : first;
}

export interface Highlight {
  id: "killer" | "spree" | "map" | "games";
  label: string;
  /** The headline number, already worded ("23 kills"). */
  value: string;
  /** Who, when it is a player's: their xuid when known, and the name shown. */
  who: { xuid: string | null; name: string } | null;
  /** A map's name and package, when it is a map's. */
  map: { map: string; map_id: string | null } | null;
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** "Since yesterday" highlights from the finished games of the last day:
 *  the top killer, the longest spree and the most played map, each only
 *  when the games have one. `games` are whatever the page has loaded. */
export function highlights(games: readonly GameState[], now: number, windowMs = 86_400_000): Highlight[] {
  const recent = games.filter((g) => g.status === "ended" && now - Date.parse(g.ended_at ?? g.started_at) <= windowMs);
  const mp = recent.filter((g) => !isCampaign(g));
  const out: Highlight[] = [];

  const key = (g: GameState, slot: number) => {
    const p = g.players[slot];
    return p ? { key: p.xuid ?? `name:${p.name}`, xuid: p.xuid ?? null, name: p.name } : null;
  };

  const kills = new Map<string, { xuid: string | null; name: string; n: number }>();
  for (const g of mp) {
    for (const [slot, s] of Object.entries(g.scores)) {
      const k = key(g, +slot);
      if (!k || !s.kills) continue;
      const row = kills.get(k.key) ?? { xuid: k.xuid, name: k.name, n: 0 };
      row.n += s.kills;
      kills.set(k.key, row);
    }
  }
  const top = [...kills.values()].sort((a, b) => b.n - a.n || a.name.localeCompare(b.name))[0];
  if (top) out.push({ id: "killer", label: "Top Killer", value: plural(top.n, "kill"), who: { xuid: top.xuid, name: top.name }, map: null });

  let spree: { xuid: string | null; name: string; n: number } | null = null;
  for (const g of mp) {
    for (const [slot, n] of bestSprees(g.kills)) {
      const k = key(g, slot);
      if (k && n >= 2 && (!spree || n > spree.n)) spree = { xuid: k.xuid, name: k.name, n };
    }
  }
  if (spree) out.push({ id: "spree", label: "Longest Spree", value: `${spree.n} in a row`, who: { xuid: spree.xuid, name: spree.name }, map: null });

  const maps = new Map<string, { map: string; map_id: string | null; n: number }>();
  for (const g of recent) {
    const k = `${g.map}|${g.map_id ?? ""}`;
    const row = maps.get(k) ?? { map: g.map, map_id: g.map_id, n: 0 };
    row.n += 1;
    maps.set(k, row);
  }
  const busiest = [...maps.values()].sort((a, b) => b.n - a.n || a.map.localeCompare(b.map))[0];
  if (busiest && busiest.n >= 2) out.push({ id: "map", label: "Most Played Map", value: plural(busiest.n, "game"), who: null, map: { map: busiest.map, map_id: busiest.map_id } });

  if (recent.length) out.push({ id: "games", label: "Games Finished", value: plural(recent.length, "game"), who: null, map: null });
  return out;
}

/** Maps for the panel's montage when there are no games at all: the
 *  week's most played (multiplayer) when the summary has any, else a few
 *  of the game's own. */
export const MONTAGE_MAPS = ["lockout", "midship", "zanzibar", "ascension", "coagulation", "beavercreek"];

export function montageMaps(week: ReadonlyArray<{ map: string; map_id?: string | null; campaign: boolean }> | null | undefined, n = 6): Array<{ map: string; map_id: string | null }> {
  const played = (week ?? []).filter((m) => !m.campaign).map((m) => ({ map: m.map, map_id: m.map_id ?? null }));
  const seen = new Set(played.map((m) => m.map));
  const rest = MONTAGE_MAPS.filter((m) => !seen.has(m)).map((map) => ({ map, map_id: null }));
  return [...played, ...rest].slice(0, n);
}
