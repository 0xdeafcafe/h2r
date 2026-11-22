import { describe, expect, test } from "bun:test";
import { clipUrl, filmListUrl, filmSaveUrl, gameClipsUrl, type FilmSummary } from "../src/api/films";
import { clipShareUrl, theaterHref } from "../src/routes";
import { ReplayClock, ReplayTimeline, playbackRange } from "../src/state/replay";
import { filmCard } from "../src/theater/cards";
import { MAX_CLIP_TICKS, MIN_CLIP_TICKS, clipRangeError } from "../src/theater/clipRange";
import { ViewerPlayback } from "../src/viewer/playback";

describe("theater clip ranges", () => {
  const bounds = { from_tick: 120, to_tick: 12_000 };

  test("requires both marks in order on recording ticks", () => {
    expect(clipRangeError({ from_tick: 120, to_tick: null }, bounds)).toContain("an out point");
    expect(clipRangeError({ from_tick: 1500, to_tick: 1500 }, bounds)).toContain("must come after");
    expect(clipRangeError({ from_tick: 1500, to_tick: 120 }, bounds)).toContain("must come after");
    expect(clipRangeError({ from_tick: 119, to_tick: 2000 }, bounds)).toContain("within");
    expect(clipRangeError({ from_tick: 120, to_tick: 12_001 }, bounds)).toContain("within");
    expect(clipRangeError({ from_tick: 120.5, to_tick: 2000 }, bounds)).toContain("within");
  });

  test("accepts thirty seconds and five minutes inclusively", () => {
    const from = 220;
    const range = (length: number) => ({ from_tick: from, to_tick: from + length });
    expect(clipRangeError(range(MIN_CLIP_TICKS - 1), bounds)).toContain("30 seconds");
    expect(clipRangeError(range(MIN_CLIP_TICKS), bounds)).toBeNull();
    expect(clipRangeError(range(MAX_CLIP_TICKS), bounds)).toBeNull();
    expect(clipRangeError(range(MAX_CLIP_TICKS + 1), bounds)).toContain("5 minutes");
  });

  const timeline = (endTick = 1500) => new ReplayTimeline([
    { game_id: "film", tick: 100, players: [] },
    { game_id: "film", tick: endTick, players: [] },
  ], []);
  const loop = { from_tick: 200, to_tick: 1100 };

  test("starts at clip in, loops with overflow, and clamps seeks to the clip", () => {
    const clock = new ReplayClock(timeline(), false, loop);
    clock.play(0);
    expect(clock.tickAt(0)).toBe(200);
    expect(clock.tickAt(30_000)).toBe(200);
    expect(clock.tickAt(91_000)).toBe(230);
    expect(clock.playing).toBe(true);
    clock.pause(91_000);
    clock.seek(1, 91_000);
    expect(clock.tickAt(91_000)).toBe(200);
    clock.seek(1500, 91_000);
    expect(clock.tickAt(91_000)).toBe(1100);
    clock.play(91_000);
    expect(clock.tickAt(91_000)).toBe(200);
  });

  test("keeps the clip bounds and speed after replacing a loaded replay window", () => {
    const playback = new ViewerPlayback(false, loop);
    playback.openRecording(timeline(), 0, "30");
    expect(playback.tickAt(0, 0)).toBe(200);
    playback.clock!.setSpeed(2, 1000);
    playback.updateRecording(timeline(1400), 2000);
    expect(playback.clock!.speed).toBe(2);
    expect(playback.tickAt(2000, 0)).toBe(290);
    expect(playback.tickAt(16_000, 0)).toBe(230);
  });

  test("uses only the portion covered by the recording", () => {
    const bounds = { startTick: 300, endTick: 1050 };
    expect(playbackRange(bounds, loop)).toEqual({ from_tick: 300, to_tick: 1050 });
    expect(playbackRange({ startTick: 1100, endTick: 1500 }, loop)).toBeUndefined();
    expect(playbackRange({ startTick: 100, endTick: 1500, recordedEndTick: 900 }, loop)).toEqual({ from_tick: 200, to_tick: 900 });
  });
});

describe("theater URLs", () => {
  const api = "https://live.example/v1";

  test("encodes game and clip IDs as individual path segments", () => {
    expect(theaterHref("a/b")).toBe("/games/halo2/games/a%2Fb#game-viewer");
    expect(theaterHref("a/b", "c #d")).toBe("/theater/a%2Fb/clip/c%20%23d");
    expect(clipShareUrl("https://live.example", "a/b", "c #d")).toBe("https://live.example/theater/a%2Fb/clip/c%20%23d");
    expect(gameClipsUrl(api, "a/b")).toBe(`${api}/films/a%2Fb/clips`);
    expect(filmSaveUrl(api, "a/b")).toBe(`${api}/films/a%2Fb/save`);
    expect(clipUrl(api, "c #d")).toBe(`${api}/clips/c%20%23d`);
  });

  test("routes film lists by player and kind", () => {
    expect(filmListUrl(api, "p+1", "saved")).toBe(`${api}/films?player=p%2B1&kind=saved`);
    expect(filmListUrl(api, "p+1", "clips")).toBe(`${api}/films?player=p%2B1&kind=clips`);
  });
});

describe("theater film cards", () => {
  const film: FilmSummary = {
    game_id: "match", map: "lockout", variant: "Team Slayer", mode: "slayer",
    started_at: "2026-11-19T12:00:00", duration_s: 487, result: "win",
  };

  test("formats map, variant, result, game length and date", () => {
    expect(filmCard(film, "Lockout")).toEqual({
      href: "/games/halo2/games/match#game-viewer", title: "Lockout", mapName: "Lockout",
      gameType: "Team Slayer", result: "Win", length: "8:07", date: "11/19/2026",
    });
  });

  test("uses the clip title and tick duration while keeping the game's result", () => {
    const clip = { id: "jump", game: film.game_id, from_tick: 600, to_tick: 1800, title: "The jump", by: "p1", created_ms: 1 };
    expect(filmCard(film, "Lockout", clip)).toMatchObject({
      href: "/theater/match/clip/jump", title: "The jump", mapName: "Lockout", result: "Win", length: "0:40",
    });
  });

  test("uses the mode when there is no variant and labels missing facts", () => {
    expect(filmCard({ ...film, variant: null, mode: "campaign", result: null, duration_s: null, started_at: "" }, "Outskirts")).toMatchObject({
      gameType: "Campaign", result: "Result unavailable", length: "Length unavailable", date: undefined,
    });
  });
});
