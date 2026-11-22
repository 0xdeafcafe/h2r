// The pure parts of Clanker Training's Watch view: which way each run's
// tile plays, the order of the tiles, the shared fast-forward clock and
// which live tiles hold one of the capped streams, and the One map overlay
// that draws a map's runs together (docs/runtime/sim.md, "Watching in
// Uplink").

import type { MapFrame } from "../components/MapCanvas";
import { clock } from "../lib/format";
import { teamName } from "../lib/game";
import type { Bounds, Marker, RosterEntry } from "../map/render";
import { TICKS_PER_SECOND } from "../state/frames";
import type { ScoreSnapshot } from "../state/replay";
import { tuningLabel, type SimRun } from "./simRuns";

/** The replay speeds the Watch view offers, and the one it opens at. */
export const WATCH_SPEEDS = [1, 4, 8, 16] as const;
export const DEFAULT_WATCH_SPEED = 8;
/** At most this many running tiles hold a live stream at once. */
export const MAX_LIVE_STREAMS = 6;
/** At most this many recordings download at once. */
export const MAX_RECORDING_READS = 3;

/** How a run's tile plays: a finished or interrupted run replays its
 *  recording; a running one streams live, unless it uploads its recording
 *  only after the match (`deferred`), when it waits. */
export type TileMode = "replay" | "live" | "waiting";

export function tileMode(run: Pick<SimRun, "status" | "sim">): TileMode {
  if (run.status !== "running") return "replay";
  return run.sim.deferred ? "waiting" : "live";
}

function seedOrder(a: string, b: string): number {
  if (/^\d+$/.test(a) && /^\d+$/.test(b)) {
    const x = BigInt(a), y = BigInt(b);
    return x < y ? -1 : x > y ? 1 : 0;
  }
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Who plays in a run: each team's tuning arms, or each bot's when the
 *  bots have no teams. */
export function armsLabel(run: Pick<SimRun, "sim">): string {
  const bots = run.sim.bots;
  if (!bots.length) return "Default";
  if (bots.every((b) => b.team === undefined)) return bots.map(tuningLabel).join(" vs ");
  const teams = new Map<number, string[]>();
  for (const b of bots) {
    const team = b.team ?? -1;
    const arms = teams.get(team) ?? [];
    const label = tuningLabel(b);
    if (!arms.includes(label)) arms.push(label);
    teams.set(team, arms);
  }
  return Array.from(teams).sort(([a], [b]) => a - b)
    .map(([team, arms]) => `${team < 0 ? "No team" : teamName(team)}: ${arms.join(", ")}`).join(" vs ");
}

/** The tiles' order: by map, then seed, then arms, so the arms played on
 *  one map and seed sit side by side. */
export function orderTiles<T extends Pick<SimRun, "map" | "sim">>(runs: readonly T[]): T[] {
  return [...runs].sort((a, b) =>
    (a.map < b.map ? -1 : a.map > b.map ? 1 : 0)
    || seedOrder(a.sim.seed, b.sim.seed)
    || (armsLabel(a) < armsLabel(b) ? -1 : armsLabel(a) > armsLabel(b) ? 1 : 0)
    || (a.sim.run_id < b.sim.run_id ? -1 : a.sim.run_id > b.sim.run_id ? 1 : 0));
}

/** The running tiles that get a live stream: the ones on screen first, in
 *  tile order, up to `cap`. The rest show their latest score until a slot
 *  frees up. */
export function streamingTiles(live: readonly string[], visible: ReadonlySet<string>, cap = MAX_LIVE_STREAMS): Set<string> {
  return new Set(live.filter((id) => visible.has(id)).slice(0, cap));
}

/** One clock for every replaying tile, so the batch plays in sync: ticks
 *  of game time since each run's first frame, at `speed` times real time.
 *  It stops at `end`, the longest loaded run, and goes on if a longer one
 *  loads after it stopped there. */
export class BatchClock {
  playing = false;
  end = 0;
  private base = 0;
  private at = 0;
  private stoppedAtEnd = false;

  constructor(public speed: number = DEFAULT_WATCH_SPEED, readonly ticksPerSecond = TICKS_PER_SECOND) {}

  elapsed(now: number): number {
    if (!this.playing) return this.base;
    const t = this.base + Math.max(0, now - this.at) / 1000 * this.ticksPerSecond * this.speed;
    if (t < this.end) return t;
    this.base = this.end;
    this.at = now;
    this.playing = false;
    this.stoppedAtEnd = true;
    return this.end;
  }

  /** Plays on, or from the start when it reached the end. */
  play(now: number): void {
    if (this.playing) return;
    if (this.base >= this.end) this.base = 0;
    this.at = now;
    this.playing = true;
    this.stoppedAtEnd = false;
  }

  pause(now: number): void {
    this.base = this.elapsed(now);
    this.playing = false;
    this.stoppedAtEnd = false;
  }

  seek(ticks: number, now: number): void {
    this.base = Math.max(0, Math.min(this.end, ticks));
    this.at = now;
  }

  setSpeed(speed: number, now: number): void {
    this.base = this.elapsed(now);
    this.at = now;
    this.speed = speed;
  }

  /** Grows the clock's end to cover a run `ticks` long. A clock waiting
   *  at the old end plays on. */
  cover(ticks: number, now: number): void {
    this.elapsed(now);
    if (ticks <= this.end) return;
    this.end = ticks;
    if (!this.stoppedAtEnd) return;
    this.at = now;
    this.playing = true;
    this.stoppedAtEnd = false;
  }
}

/** A replaying tile's tick on the shared clock: its own start plus the
 *  clock's elapsed ticks, held on its last frame once it has ended. */
export function tileTick(elapsed: number, startTick: number, endTick: number): number {
  return Math.min(endTick, startTick + Math.max(0, elapsed));
}

/** The tile's game clock, as m:ss. */
export function gameClock(ticks: number, ticksPerSecond = TICKS_PER_SECOND): string {
  return clock(ticks / ticksPerSecond);
}

/** The score at a replay moment: the teams' scores in team order, else
 *  each player's in slot order. */
export function scoreAt(snapshot: Pick<ScoreSnapshot, "players" | "teams">, teamPlay: boolean): string {
  const teams = Object.entries(snapshot.teams).map(([team, score]) => [Number(team), score] as const);
  if (teamPlay && teams.length) return teams.sort(([a], [b]) => a - b).map(([, s]) => s).join("–");
  const players = Object.values(snapshot.players).sort((a, b) => a.index - b.index);
  return players.map((p) => p.score).join("–") || "—";
}

/** Starts `read`s at most `limit` at a time, in the order asked. */
export function limiter(limit: number): <T>(read: () => Promise<T>) => Promise<T> {
  let active = 0;
  const queue: (() => void)[] = [];
  const next = () => {
    if (active >= limit) return;
    const start = queue.shift();
    if (start) { active++; start(); }
  };
  return <T>(read: () => Promise<T>) => new Promise<T>((resolve, reject) => {
    queue.push(() => {
      read().then(resolve, reject).finally(() => { active--; next(); });
    });
    next();
  });
}

/** The batch's runs grouped by map, each group in tile order: one One map
 *  viewer per group. */
export function groupByMap<T extends Pick<SimRun, "map" | "sim">>(runs: readonly T[]): { map: string; runs: T[] }[] {
  const groups: { map: string; runs: T[] }[] = [];
  for (const run of orderTiles(runs)) {
    const last = groups.at(-1);
    if (last && last.map === run.map) last.runs.push(run); else groups.push({ map: run.map, runs: [run] });
  }
  return groups;
}

/** The colours the overlay gives its runs, in tile order. They wrap after
 *  the last one. */
export const RUN_COLOURS = [
  "#4aa3ff", "#ff6b4a", "#5fd35f", "#ffd23f", "#c77dff", "#2ee6d6",
  "#ff8fc7", "#ff9f1c", "#a0e426", "#9aa8ff", "#d9d9d9", "#b5651d",
] as const;

export function runColour(slot: number): string {
  return RUN_COLOURS[((slot % RUN_COLOURS.length) + RUN_COLOURS.length) % RUN_COLOURS.length]!;
}

/** How strongly the runs that are not highlighted are drawn. */
export const DIM_ALPHA = 0.25;

/** `colour` at `alpha`, as the renderers' fill and stroke styles take it.
 *  A colour that is not a hex one is returned as it is. */
export function dimColour(colour: string, alpha = DIM_ALPHA): string {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(colour);
  if (!m) return colour;
  const hex = m[1]!.length === 3 ? m[1]!.replace(/./g, (c) => c + c) : m[1]!;
  const n = parseInt(hex, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}

/** Player indices are small, so each run's players are keyed
 *  `slot * PLAYER_STRIDE + index` in the merged frame and roster. */
export const PLAYER_STRIDE = 1000;

export function overlayKey(slot: number, index: number): number {
  return slot * PLAYER_STRIDE + index;
}

/** One run's share of an overlay: its slot, name and colour, whether it is
 *  drawn dimmed, its roster and its frame at this moment. */
export interface OverlayPart {
  slot: number;
  label: string;
  colour: string;
  dim: boolean;
  roster: ReadonlyMap<number, RosterEntry>;
  frame: MapFrame;
}

/** One roster for the overlay: every run's players under their own key, in
 *  the run's colour (dimmed, when the run is) and named after the run. */
export function mergeRosters(parts: readonly Pick<OverlayPart, "slot" | "label" | "colour" | "dim" | "roster">[]): Map<number, RosterEntry> {
  const merged = new Map<number, RosterEntry>();
  for (const part of parts) {
    const colour = part.dim ? dimColour(part.colour) : part.colour;
    for (const [index, entry] of part.roster) {
      merged.set(overlayKey(part.slot, index), { ...entry, name: `${part.label} · ${entry.name}`, colour });
    }
  }
  return merged;
}

function union(a: Bounds | null, b: Bounds | null): Bounds | null {
  if (!a || !b) return a ?? b;
  return { minX: Math.min(a.minX, b.minX), minY: Math.min(a.minY, b.minY), maxX: Math.max(a.maxX, b.maxX), maxY: Math.max(a.maxY, b.maxY) };
}

/** One frame for the overlay: every run's players re-keyed as in
 *  `mergeRosters`, its death marks in the run's colour (faded when the run
 *  is dimmed) and the bounds all of them have covered. */
export function mergeFrames(parts: readonly Pick<OverlayPart, "slot" | "colour" | "dim" | "frame">[]): MapFrame {
  const players: MapFrame["players"] = [];
  const markers: Marker[] = [];
  let seen: Bounds | null = null;
  for (const { slot, colour, dim, frame } of parts) {
    for (const p of frame.players) players.push({ ...p, index: overlayKey(slot, p.index) });
    for (const m of frame.markers) markers.push({ ...m, colour, alpha: dim ? m.alpha * DIM_ALPHA : m.alpha });
    seen = union(seen, frame.seen);
  }
  return { players, markers, seen };
}
