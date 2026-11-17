import { describe, expect, test } from "bun:test";
import type { Frame } from "../src/api/types";
import { FrameBuffer, findFrame, interpolateFrames, lerpAngle, sampleFrames, sampleObjects, sampleVehicles } from "../src/state/frames";

const f = (tick: number, players: Array<[number, number, number, boolean?, number?]>): Frame => ({
  game_id: "g", tick,
  players: players.map(([index, x, y, alive = true, yaw = 0]) => ({ index, alive, pos: [x, y, 0], yaw })),
});

describe("interpolation", () => {
  test("lerps position halfway", () => {
    const out = interpolateFrames(f(0, [[0, 0, 0]]), f(10, [[0, 10, -4]]), 5);
    expect(out[0]!.x).toBeCloseTo(5);
    expect(out[0]!.y).toBeCloseTo(-2);
  });

  test("angles take the short way round", () => {
    const a = Math.PI - 0.1, b = -Math.PI + 0.1;
    const mid = lerpAngle(a, b, 0.5);
    expect(Math.abs(Math.abs(mid) - Math.PI)).toBeLessThan(1e-9);
    expect(lerpAngle(0, 1, 0.25)).toBeCloseTo(0.25);
  });

  test("snaps on respawn or long jumps instead of sliding", () => {
    const early = interpolateFrames(f(0, [[0, 0, 0]]), f(10, [[0, 100, 0]]), 3);
    expect(early[0]!.x).toBe(0);
    const late = interpolateFrames(f(0, [[0, 0, 0]]), f(10, [[0, 100, 0]]), 7);
    expect(late[0]!.x).toBe(100);
    const dead = interpolateFrames(f(0, [[0, 0, 0, false]]), f(10, [[0, 2, 0, true]]), 2);
    expect(dead[0]!.alive).toBe(false);
  });

  test("players appearing or vanishing", () => {
    const out = interpolateFrames(f(0, [[0, 0, 0], [1, 5, 5]]), f(10, [[0, 1, 0], [2, 9, 9]]), 6);
    expect(out.map((p) => p.index).sort()).toEqual([0, 1, 2]);
    expect(out.find((p) => p.index === 1)!.x).toBe(5);
    const early = interpolateFrames(f(0, [[0, 0, 0]]), f(10, [[0, 1, 0], [2, 9, 9]]), 2);
    expect(early.map((p) => p.index)).toEqual([0]);
  });

  test("findFrame and sampleFrames clamp at the ends", () => {
    const frames = [f(10, [[0, 0, 0]]), f(20, [[0, 10, 0]]), f(30, [[0, 20, 0]])];
    expect(findFrame(frames, 5)).toBe(-1);
    expect(findFrame(frames, 20)).toBe(1);
    expect(findFrame(frames, 99)).toBe(2);
    expect(sampleFrames(frames, 0)[0]!.x).toBe(0);
    expect(sampleFrames(frames, 25)[0]!.x).toBeCloseTo(15);
    expect(sampleFrames(frames, 99)[0]!.x).toBe(20);
    expect(sampleFrames([], 5)).toEqual([]);
  });
});

describe("FrameBuffer", () => {
  test("renders behind the newest frame so 15 Hz looks smooth", () => {
    const b = new FrameBuffer(48, 4);
    // 15 Hz: 2 ticks per frame, ~66.7 ms apart.
    for (let i = 0; i <= 10; i++) b.push(f(i * 2, [[0, i * 2, 0]]), 1000 + i * (2000 / 30));
    const now = 1000 + 10 * (2000 / 30);
    const t = b.renderTick(now);
    expect(t).toBeCloseTo(16, 0);
    // A few ms later the render tick has moved on, between frames.
    const t2 = b.renderTick(now + 20);
    expect(t2).toBeGreaterThan(t);
    expect(t2).toBeLessThan(20);
    const x = b.sample(now + 20)[0]!.x;
    expect(x).toBeGreaterThan(16);
    expect(x).toBeLessThan(18);
  });

  test("never extrapolates past the newest frame", () => {
    const b = new FrameBuffer(48, 4);
    b.push(f(0, [[0, 0, 0]]), 0);
    b.push(f(2, [[0, 2, 0]]), 66);
    expect(b.renderTick(10_000)).toBe(2);
  });

  test("drops stale frames, keeps capacity, resets on tick restart", () => {
    const b = new FrameBuffer(4, 2);
    for (let i = 0; i < 10; i++) b.push(f(i * 2, [[0, i, 0]]), i * 66);
    expect(b.frames).toHaveLength(4);
    b.push(f(4, [[0, 0, 0]]), 700); // older than newest, within 5s: ignored
    expect(b.lastTick).toBe(18);
    b.push(f(0, [[0, 0, 0]]), 800);
    expect(b.lastTick).toBe(18);
    // A restarted game far behind the newest tick starts over.
    const c = new FrameBuffer(4, 2);
    c.push(f(1000, [[0, 0, 0]]), 0);
    c.push(f(2, [[0, 0, 0]]), 100);
    expect(c.lastTick).toBe(2);
  });

  test("tracks seen bounds and death markers", () => {
    const b = new FrameBuffer();
    b.push(f(0, [[0, -5, 3], [1, 7, -2]]), 0);
    b.push(f(2, [[0, -4, 3], [1, 8, -2]]), 66);
    expect(b.bounds.minX).toBe(-5);
    expect(b.bounds.maxX).toBe(8);
    b.addDeath(1, 2, 100);
    expect(b.liveMarkers(200, 1000)).toHaveLength(1);
    expect(b.liveMarkers(200, 1000)[0]!.x).toBe(8);
    expect(b.liveMarkers(2000, 1000)).toHaveLength(0);
  });
});

describe("per-frame caches", () => {
  const veh = (id: string, x: number, parent?: string) => ({
    id, kind: "warthog", pos: [x, 5, 5] as [number, number, number], forward: [1, 0, 0] as [number, number, number],
    up: [0, 0, 1] as [number, number, number], occupied: true, ...(parent ? { parent } : {}),
  });

  test("seating a turret does not change the frame's cached vehicles", () => {
    const f1: Frame = { game_id: "g", tick: 0, players: [], vehicles: [veh("hog", 0), veh("gun", 5, "hog")] };
    const f2: Frame = { game_id: "g", tick: 10, players: [], vehicles: [veh("gun", 5, "hog")] };
    // Seated on the hog in the first frame alone, every time.
    for (let k = 0; k < 2; k++) expect(sampleVehicles([f1], 0).find((v) => v.id === "gun")!.x).toBe(0);
    // Past halfway the hog is gone: the turret is where both frames have it.
    expect(sampleVehicles([f1, f2], 7.5).find((v) => v.id === "gun")!.x).toBe(5);
  });

  test("repeated samples give equal, separate lists", () => {
    const f1: Frame = { game_id: "g", tick: 0, players: [], vehicles: [veh("a", 0)], objects: [{ k: "scen", i: 1, p: [1, 2, 3] }] };
    const f2: Frame = { game_id: "g", tick: 10, players: [], vehicles: [veh("a", 1), veh("b", 2)], objects: [{ k: "scen", i: 1, p: [2, 2, 3] }] };
    const one = sampleVehicles([f1, f2], 6), two = sampleVehicles([f1, f2], 6);
    expect(one).toEqual(two);
    expect(one).not.toBe(two);
    expect(one.map((v) => v.id)).toEqual(["a", "b"]);
    expect(one[0]!.x).toBeCloseTo(0.6);
    expect(sampleObjects([f1, f2], 5)![0]!.p![0]).toBeCloseTo(1.5);
    expect(sampleObjects([f1, f2], 5)).toEqual(sampleObjects([f1, f2], 5));
  });
});
