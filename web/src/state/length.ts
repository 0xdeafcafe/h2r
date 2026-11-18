// How long a game was: on the game's own clock (ticks) or on the wall clock.

import type { GameState } from "./reducer";
import { TICKS_PER_SECOND } from "./frames";

/** Wall-clock time from GameStarted to GameEnded (or now). For a game that
 *  ended as `lost` this includes the server's idle wait (5 minutes) before
 *  it gave up on the console, and it counts time the game ran slower than
 *  real time. */
export function elapsedSeconds(g: GameState, now: number): number {
  const end = g.ended_at ? Date.parse(g.ended_at) : now;
  return Math.max(0, (end - Date.parse(g.started_at)) / 1000);
}

/** How long the match itself ran, in game time: the game's tick count at 30
 *  ticks a second, the clock the replay timeline uses. Falls back to the
 *  wall clock while live or when no tick after the start is known, except
 *  for a `lost` game: one that never reported a tick (it went quiet before
 *  its first frame) played nothing the server saw, and its wall clock is
 *  only the server's idle wait, so it counts as no time at all. */
export function gameSeconds(g: GameState, now: number): number {
  if (g.status === "ended" && g.last_tick > g.start_tick) return (g.last_tick - g.start_tick) / TICKS_PER_SECOND;
  if (g.status === "ended" && g.end_reason === "lost") return 0;
  return elapsedSeconds(g, now);
}
