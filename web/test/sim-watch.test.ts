import { describe, expect, test } from "bun:test";
import type { Frame } from "../src/api/types";
import {
  BatchClock, DEFAULT_WATCH_SPEED, MAX_LIVE_STREAMS, WATCH_SPEEDS, armsLabel, gameClock, limiter, orderTiles, scoreAt,
  streamingTiles, tileMode, tileTick,
} from "../src/admin/batchWatch";
import type { SimBot, SimRun } from "../src/admin/simRuns";
import { FrameBuffer, MAX_STREAM_PACE, TICKS_PER_SECOND, streamPace } from "../src/state/frames";
import { ReplayTimeline } from "../src/state/replay";
import { ViewerPlayback } from "../src/viewer/playback";
import { SPEEDS } from "../src/viewer/session";

function run(id: string, extra: { map?: string; seed?: string; bots?: SimBot[]; status?: SimRun["status"]; deferred?: boolean } = {}) {
  return {
    game_id: "sim-" + id, map: extra.map ?? "lockout", status: extra.status ?? "finished",
    sim: { run_id: id, batch_id: "b", gametype: "slayer", seed: extra.seed ?? "1", pace: "unpaced",
      bots: extra.bots ?? [{ arm: "a" }], deferred: extra.deferred ?? false },
  };
}

describe("batch watch tiles", () => {
  test("finished and interrupted runs replay, running ones stream live unless deferred", () => {
    expect(tileMode(run("f"))).toBe("replay");
    expect(tileMode(run("i", { status: "interrupted" }))).toBe("replay");
    expect(tileMode(run("r", { status: "running" }))).toBe("live");
    expect(tileMode(run("d", { status: "running", deferred: true }))).toBe("waiting");
  });

  test("tiles order by map, then seed as a number, then arms", () => {
    const tiles = orderTiles([
      run("5", { map: "midship", seed: "2" }),
      run("4", { map: "lockout", seed: "10", bots: [{ arm: "b" }] }),
      run("3", { map: "lockout", seed: "10", bots: [{ arm: "a" }] }),
      run("2", { map: "lockout", seed: "9" }),
      run("1", { map: "ascension", seed: "18446744073709551615" }),
    ]);
    expect(tiles.map((t) => t.sim.run_id)).toEqual(["1", "2", "3", "4", "5"]);
  });

  test("arms read per team when the bots have teams", () => {
    expect(armsLabel(run("x", { bots: [{ arm: "aggressive" }, { skill: "hard" }] }))).toBe("aggressive vs hard");
    expect(armsLabel(run("y", { bots: [
      { team: 1, arm: "control" }, { team: 0, arm: "push" }, { team: 0, arm: "push" }, { team: 1 },
    ] }))).toBe("Red: push vs Blue: control, Default");
    expect(armsLabel(run("z", { bots: [] }))).toBe("Default");
  });

  test("live streams go to on-screen tiles first, up to the cap", () => {
    const live = Array.from({ length: 10 }, (_, i) => `sim-${i}`);
    const visible = new Set(live.slice(2));
    const chosen = streamingTiles(live, visible);
    expect(chosen.size).toBe(MAX_LIVE_STREAMS);
    expect([...chosen]).toEqual(live.slice(2, 2 + MAX_LIVE_STREAMS));
    expect(streamingTiles(live, new Set(), 3).size).toBe(0);
    expect([...streamingTiles(live, new Set(["sim-9", "sim-1"]), 1)]).toEqual(["sim-1"]);
  });

  test("the speed choices are 1×, 4×, 8× and 16×, opening at 8×", () => {
    expect([...WATCH_SPEEDS]).toEqual([1, 4, 8, 16]);
    expect(DEFAULT_WATCH_SPEED).toBe(8);
    expect(SPEEDS).toContain(16);
  });
});

describe("the batch clock", () => {
  test("plays game time at its speed and changes speed without a jump", () => {
    const clock = new BatchClock(8);
    clock.cover(100_000, 0);
    clock.play(0);
    expect(clock.elapsed(1000)).toBe(8 * TICKS_PER_SECOND);
    clock.setSpeed(16, 1000);
    expect(clock.elapsed(1000)).toBe(240);
    expect(clock.elapsed(1500)).toBe(240 + 16 * 15);
    clock.pause(1500);
    expect(clock.elapsed(9000)).toBe(480);
  });

  test("waits at the end of the longest run and plays on when a longer one loads", () => {
    const clock = new BatchClock(4);
    clock.play(0);
    // Nothing loaded: the clock waits at zero.
    expect(clock.elapsed(500)).toBe(0);
    expect(clock.playing).toBe(false);
    clock.cover(300, 500);
    expect(clock.playing).toBe(true);
    expect(clock.elapsed(500 + 1000)).toBe(120);
    expect(clock.elapsed(500 + 5000)).toBe(300);
    expect(clock.playing).toBe(false);
    clock.cover(600, 6000);
    expect(clock.elapsed(6000 + 1000)).toBe(420);
    // Play after the end starts over.
    expect(clock.elapsed(20_000)).toBe(600);
    clock.play(20_000);
    expect(clock.elapsed(20_000)).toBe(0);
  });

  test("a paused clock stays paused when a longer run loads", () => {
    const clock = new BatchClock(1);
    clock.cover(30, 0);
    clock.play(0);
    clock.pause(500);
    clock.cover(900, 600);
    expect(clock.playing).toBe(false);
    expect(clock.elapsed(5000)).toBe(15);
  });

  test("each tile holds its last frame once its run has ended", () => {
    expect(tileTick(50, 1000, 4000)).toBe(1050);
    expect(tileTick(9000, 1000, 4000)).toBe(4000);
    expect(tileTick(-5, 1000, 4000)).toBe(1000);
    expect(gameClock(75 * TICKS_PER_SECOND)).toBe("1:15");
  });

  test("the score reads teams in team order, else players in slot order", () => {
    const players = {
      1: { index: 1, score: 3, kills: 3, deaths: 0, assists: 0 },
      0: { index: 0, score: 5, kills: 5, deaths: 1, assists: 0 },
    };
    expect(scoreAt({ players, teams: { 1: 2, 0: 7 } }, true)).toBe("7–2");
    expect(scoreAt({ players, teams: {} }, false)).toBe("5–3");
    expect(scoreAt({ players: {}, teams: {} }, false)).toBe("—");
  });

  test("recordings download a few at a time", async () => {
    const run = limiter(2);
    let active = 0, most = 0;
    const releases: (() => void)[] = [];
    const reads = Array.from({ length: 5 }, (_, i) => run(() => {
      active++;
      most = Math.max(most, active);
      return new Promise<number>((resolve) => releases.push(() => { active--; resolve(i); }));
    }));
    await Promise.resolve();
    expect(releases).toHaveLength(2);
    while (releases.length) {
      releases.shift()!();
      await new Promise((r) => setTimeout(r, 0));
    }
    expect(await Promise.all(reads)).toEqual([0, 1, 2, 3, 4]);
    expect(most).toBe(2);
  });
});

describe("live and replay speed", () => {
  const frame = (tick: number): Frame => ({ game_id: "sim-fast", tick, players: [{ index: 0, alive: true, pos: [tick, 0, 0], yaw: 0 }] });

  test("a stream's pace is measured from its arrivals", () => {
    const at = (ticksPerBatch: number, n = 30) => Array.from({ length: n }, (_, i) => ({ tick: i * ticksPerBatch, at: i * 2000 / 30 }));
    expect(streamPace(at(2))).toBe(1);
    expect(streamPace(at(6))).toBe(3);
    expect(streamPace(at(16))).toBe(8);
    expect(streamPace(at(500))).toBe(MAX_STREAM_PACE);
    // Too short a span to trust.
    expect(streamPace(at(16, 3))).toBe(1);
  });

  test("a sim streaming at 8× plays smoothly at that pace near its live edge", () => {
    const b = new FrameBuffer(48, 5);
    const step = 2000 / 30;
    for (let i = 0; i <= 90; i++) b.push(frame(i * 16), i * step);
    const now = 90 * step;
    expect(b.pace).toBe(8);
    const edge = b.renderTick(now);
    expect(edge).toBeGreaterThan(b.lastTick - 5 * 8 - 16);
    // Between frames the clock moves at the stream's pace, not 1×.
    const later = b.renderTick(now + 30);
    expect(later - edge).toBeCloseTo(30 / (1000 / 30) * 8, 0);
  });

  test("a real-time game keeps the 1× clock", () => {
    const b = new FrameBuffer(48, 5);
    for (let i = 0; i <= 90; i++) b.push(frame(i * 2), i * 2000 / 30);
    expect(b.pace).toBe(1);
  });

  test("a sim replay opens at its asked speed and keeps the viewer's choice after", () => {
    const frames = Array.from({ length: 61 }, (_, s) => frame(s * TICKS_PER_SECOND));
    const playback = new ViewerPlayback(false, undefined, DEFAULT_WATCH_SPEED);
    playback.openRecording(new ReplayTimeline(frames, []), 1000);
    expect(playback.clock!.speed).toBe(8);
    expect(playback.tickAt(2000, 0)).toBe(8 * TICKS_PER_SECOND);
    playback.clock!.setSpeed(16, 2000);
    playback.updateRecording(new ReplayTimeline(frames, []), 2000);
    expect(playback.clock!.speed).toBe(16);
    expect(new ViewerPlayback(false).speed).toBe(1);
  });
});
