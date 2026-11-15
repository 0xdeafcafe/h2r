import { describe, expect, test } from "bun:test";
import type { Frame } from "../src/api/types";
import { ReplayClock, ReplayTimeline } from "../src/state/replay";
import { ev, resetPos } from "./helpers";

const frame = (tick: number, x: number, alive = true): Frame => ({
  game_id: "g", tick, players: [{ index: 0, alive, pos: [x, 0, 0], yaw: 0 }, { index: 1, alive: true, pos: [0, x, 0], yaw: 0 }],
});

function timeline() {
  resetPos();
  const frames = [frame(0, 0), frame(30, 3), frame(60, 6), frame(90, 9)];
  const events = [
    ev("ScoreChanged", "game-g", { tick: 40, players: [{ index: 1, score: 1, kills: 1, deaths: 0, assists: 0 }] }),
    ev("PlayerKilled", "game-g", { tick: 40, victim: 0, killer: 1, kind: "kill" }),
    ev("ScoreChanged", "game-g", { tick: 80, players: [{ index: 1, score: 2, kills: 2, deaths: 0, assists: 0 }] }),
    ev("PlayerKilled", "game-g", { tick: 80, victim: 0, killer: 1, kind: "kill" }),
    ev("GameEnded", "game-g", { tick: 120, reason: "finished", winner: 1 }),
  ];
  // Out of order on purpose: the timeline sorts.
  return new ReplayTimeline([frames[2]!, frames[0]!, frames[3]!, frames[1]!], events);
}

describe("ReplayTimeline", () => {
  test("span covers frames and the final event", () => {
    const t = timeline();
    expect(t.startTick).toBe(0);
    expect(t.endTick).toBe(120);
    expect(t.durationSeconds).toBe(4);
  });

  test("samples between 1 Hz frames", () => {
    const t = timeline();
    expect(t.sample(15).find((p) => p.index === 1)!.y).toBeCloseTo(1.5);
    expect(t.sample(-50).find((p) => p.index === 1)!.y).toBe(0);
    expect(t.sample(500).find((p) => p.index === 1)!.y).toBe(9);
  });

  test("victims are down right after a kill even if frames lag", () => {
    const t = timeline();
    expect(t.sample(45).find((p) => p.index === 0)!.alive).toBe(false);
    expect(t.sample(75).find((p) => p.index === 0)!.alive).toBe(true);
  });

  test("kill positions come from the frames", () => {
    const t = timeline();
    expect(t.kills).toHaveLength(2);
    expect(t.kills[0]!.at!.x).toBeCloseTo(4);
  });

  test("scoreboard at a tick", () => {
    const t = timeline();
    expect(t.scoresAt(10).players[1]).toBeUndefined();
    expect(t.scoresAt(40).players[1]!.score).toBe(1);
    expect(t.scoresAt(100).players[1]!.score).toBe(2);
    expect(t.killsBetween(-Infinity, 79)).toHaveLength(1);
    expect(t.killsBetween(40, 80)).toHaveLength(1);
  });

  test("GameFrame events are merged with fetched frames", () => {
    resetPos();
    const t = new ReplayTimeline([frame(0, 0)], [ev("GameFrame", "game-g", frame(30, 3)), ev("GameFrame", "game-g", frame(0, 99))]);
    expect(t.frames.map((f) => f.tick)).toEqual([0, 30]);
    expect(t.frames[0]!.players[0]!.pos[0]).toBe(0);
  });
});

describe("ReplayClock", () => {
  test("play, pause, speed, seek, and stop at the end", () => {
    const c = new ReplayClock(timeline());
    expect(c.tickAt(0)).toBe(0);
    c.play(1000);
    expect(c.tickAt(2000)).toBeCloseTo(30);
    c.setSpeed(2, 2000);
    expect(c.tickAt(2500)).toBeCloseTo(60);
    c.pause(2500);
    expect(c.tickAt(9999)).toBeCloseTo(60);
    c.seek(-10, 3000);
    expect(c.tickAt(3000)).toBe(0);
    c.seek(110, 3000);
    c.play(3000);
    expect(c.tickAt(10_000)).toBe(120);
    expect(c.playing).toBe(false);
    // Play again from the end restarts.
    c.play(11_000);
    expect(c.tickAt(11_000)).toBe(0);
  });
});
