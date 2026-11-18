// The stats home's quiet-time pieces: the "last game" loop and highlights
// in the Live Now panel (src/lib/lastGame.ts), and the menu column's
// daily chart, zero-free rows and week picks (src/lib/statsHome.ts).

import { describe, expect, test } from "bun:test";
import {
  LOOP_GAME_SECONDS, LOOP_HOLD_MS, LOOP_WALL_SECONDS, LoopClock, MONTAGE_MAPS, highlights, loopSpan, montageMaps, pickLastGame,
} from "../src/lib/lastGame";
import { alsoPopular, dailyGames, missionOfTheWeek, nonZeroRows, ofTheWeek, type MapRow } from "../src/lib/statsHome";
import type { GameState, Kill } from "../src/state/reducer";

const TPS = 30;
const NOW = Date.parse("2026-11-18T20:00:00Z");
const ago = (min: number) => new Date(NOW - min * 60_000).toISOString();

let pos = 0;
function game(id: string, o: Partial<GameState> = {}): GameState {
  return {
    game_id: id, console: null, host: null, map: "lockout", map_id: null, mode: "multiplayer", kind: "custom", playlist: null,
    playlist_id: null, difficulty: null, online: null, skulls: [], variant: "slayer", team_play: false, score_limit: 25,
    time_limit_s: null, started_at: ago(30), ended_at: ago(20), status: "ended", end_reason: "finished", winner: null,
    start_tick: 0, players: {}, scores: {}, teams: {}, kills: [], last_tick: 0, first_position: 0, positions: [], ...o,
  };
}
const player = (index: number, name: string, xuid: string | null = null) =>
  ({ index, name, xuid, team: 0, game_name: null, left: false, appearance: null });
const score = (index: number, kills: number) => ({ index, score: kills, kills, deaths: 0, assists: 0 });
const kill = (tick: number, killer: number | null, victim: number): Kill =>
  ({ position: ++pos, at: "", tick, killer, victim, weapon: "battle_rifle", kind: "kill", pos: null });

describe("the loop over the last game", () => {
  test("loops the closing stretch, sped up to about half a minute", () => {
    const s = loopSpan(0, 600 * TPS, TPS)!;
    expect(s.to).toBe(600 * TPS);
    expect(s.from).toBe((600 - LOOP_GAME_SECONDS) * TPS);
    expect(s.speed).toBeCloseTo(LOOP_GAME_SECONDS / LOOP_WALL_SECONDS);
  });

  test("a short game plays whole and never slower than real time", () => {
    const s = loopSpan(100, 100 + 20 * TPS, TPS)!;
    expect(s).toEqual({ from: 100, to: 100 + 20 * TPS, speed: 1 });
  });

  test("an empty recording has nothing to loop", () => {
    expect(loopSpan(50, 50, TPS)).toBeNull();
  });

  test("the clock runs through the span, holds the end, then starts over", () => {
    const c = new LoopClock({ from: 0, to: 10 * TPS, speed: 2 }, TPS);
    expect(c.tickAt(1000)).toBe(0); // not playing yet
    c.play(1000);
    expect(c.tickAt(3500)).toBeCloseTo(5 * TPS); // 2.5 s at 2x
    expect(c.tickAt(6000 + LOOP_HOLD_MS / 2)).toBe(10 * TPS); // the hold
    expect(c.tickAt(6000 + LOOP_HOLD_MS + 500)).toBeCloseTo(1 * TPS); // round again
  });

  test("paused off-screen, it picks up where it stopped", () => {
    const c = new LoopClock({ from: 0, to: 60 * TPS, speed: 1 }, TPS);
    c.play(0);
    c.pause(2000);
    expect(c.running).toBe(false);
    expect(c.tickAt(50_000)).toBeCloseTo(2 * TPS);
    c.play(50_000);
    expect(c.tickAt(51_000)).toBeCloseTo(3 * TPS);
  });

  test("the newest finished game, multiplayer preferred over a campaign run from the same hour", () => {
    const camp = game("c", { mode: "campaign", ended_at: ago(5) });
    const mp = game("m", { ended_at: ago(25) });
    const live = game("l", { status: "live", ended_at: null });
    expect(pickLastGame([live, camp, mp])!.game_id).toBe("m");
    expect(pickLastGame([camp, game("old", { ended_at: ago(200) })])!.game_id).toBe("c");
    expect(pickLastGame([live])).toBeNull();
  });
});

describe("since yesterday", () => {
  test("top killer, longest spree, most played map and the count, from the last day's games", () => {
    const a = game("a", {
      players: { 0: player(0, "Mochi", "x0"), 1: player(1, "Noodle", "x1") },
      scores: { 0: score(0, 7), 1: score(1, 3) },
      kills: [kill(1, 1, 0), kill(2, 0, 1), kill(3, 0, 1), kill(4, 0, 1), kill(5, 1, 0)],
    });
    const b = game("b", { players: { 0: player(0, "Noodle", "x1") }, scores: { 0: score(0, 6) }, ended_at: ago(60) });
    const old = game("old", { map: "midship", players: { 0: player(0, "Pixel") }, scores: { 0: score(0, 99) }, ended_at: ago(60 * 30) });
    const hl = highlights([a, b, old], NOW);
    expect(hl.map((h) => h.id)).toEqual(["killer", "spree", "map", "games"]);
    expect(hl[0]).toMatchObject({ value: "9 kills", who: { xuid: "x1", name: "Noodle" } });
    expect(hl[1]).toMatchObject({ value: "3 in a row", who: { name: "Mochi" } });
    expect(hl[2]).toMatchObject({ value: "2 games", map: { map: "lockout" } });
    expect(hl[3]!.value).toBe("2 games");
  });

  test("nothing to say leaves the highlights out rather than showing zeros", () => {
    expect(highlights([], NOW)).toEqual([]);
    const quiet = game("q", { mode: "campaign", players: { 0: player(0, "Mochi") } });
    expect(highlights([quiet], NOW).map((h) => h.id)).toEqual(["games"]);
  });

  test("the montage leads with the week's multiplayer maps, then the game's own", () => {
    const m = montageMaps([{ map: "orchard", campaign: false }, { map: "harbor", campaign: true }, { map: "lockout", map_id: "pkg", campaign: false }]);
    expect(m.slice(0, 2)).toEqual([{ map: "orchard", map_id: null }, { map: "lockout", map_id: "pkg" }]);
    expect(m).toHaveLength(6);
    expect(m.filter((x) => x.map === "lockout")).toHaveLength(1);
    expect(montageMaps(null).map((x) => x.map)).toEqual(MONTAGE_MAPS.slice(0, 6));
  });
});

describe("the menu column on a young server", () => {
  test("games per day: fourteen of the viewer's days, today last", () => {
    const now = new Date(2026, 10, 18, 15, 0).getTime();
    const at = (d: number, h: number) => new Date(2026, 10, d, h).toISOString();
    const days = dailyGames([
      { hour: at(18, 9), games: 2, kills: 0 }, { hour: at(18, 13), games: 1, kills: 0 },
      { hour: at(17, 22), games: 4, kills: 0 }, { hour: at(1, 12), games: 9, kills: 0 },
    ], now);
    expect(days).toHaveLength(14);
    expect(days.at(-1)!.games).toBe(3);
    expect(days.at(-2)!.games).toBe(4);
    expect(days.reduce((a, d) => a + d.games, 0)).toBe(7); // the 1st is out of range
    expect(days[0]!.day.getDate()).toBe(5);
  });

  test("zero rows drop out, the ones marked always stay, and the cap never drops those", () => {
    const rows = nonZeroRows([
      { label: "Games", n: 3, always: true }, { label: "Kills", n: 0 }, { label: "Campaign", n: 3 },
      { label: "Hours", n: 2 }, { label: "Legendary", n: 0 }, { label: "Accounts", n: 0, always: true },
    ], 3);
    expect(rows).toEqual([{ label: "Games", value: "3" }, { label: "Campaign", value: "3" }, { label: "Accounts", value: "0" }]);
    expect(nonZeroRows([{ label: "K/D", n: 5, text: "1.50" }], 4)).toEqual([{ label: "K/D", value: "1.50" }]);
  });

  test("a week with no kills names a mission instead of a weapon; the map has runners-up", () => {
    const maps: MapRow[] = [
      { map: "bloodgulch", campaign: false, games: 11, kills: 0 },
      { map: "a10", campaign: true, games: 4, kills: 0 },
      { map: "a30", campaign: true, games: 6, kills: 0 },
      { map: "lockout", campaign: false, games: 2, kills: 0 },
    ];
    const pick = ofTheWeek({ weapons: [], maps });
    expect(pick.weapon).toBeNull();
    expect(missionOfTheWeek({ maps })!.map).toBe("a30");
    expect(alsoPopular({ maps }, pick.map).map((m) => m.map)).toEqual(["lockout"]);
    expect(missionOfTheWeek({ maps: maps.filter((m) => !m.campaign) })).toBeNull();
  });
});
