import { describe, expect, test } from "bun:test";
import type { Frame } from "../src/api/types";
import { ReplayClock, ReplayTimeline } from "../src/state/replay";
import { TICKS_PER_SECOND, type SampledPlayer } from "../src/state/frames";
import { applyClock, clockAgrees, isSyncMessage, readClock, smoothPlayers, syncChannelName, type SmoothPlayer } from "../src/viewer/sync";
import { FOLLOW_DISTANCE, cameraFit, followDistance, markerScale } from "../src/viewer/camera";
import { MIN_H, MIN_W, placeWindow } from "../src/viewer/window";

function frames(seconds: number): Frame[] {
  return Array.from({ length: seconds + 1 }, (_, s) => ({
    game_id: "g", tick: s * TICKS_PER_SECOND,
    players: [{ index: 0, alive: true, pos: [s * 2, 0, 0] as [number, number, number], yaw: 0 }],
  }));
}
const timeline = () => new ReplayTimeline(frames(60), []);

describe("timeline sync", () => {
  test("a second clock put into the first one's state shows the same tick from then on", () => {
    const a = new ReplayClock(timeline()), b = new ReplayClock(timeline());
    a.seek(300, 1000);
    a.setSpeed(2, 1000);
    a.play(1000);
    // The other window hears about it half a second later, on its own time base.
    const state = readClock(a, 1500);
    expect(state).toEqual({ tick: 300 + 0.5 * TICKS_PER_SECOND * 2, playing: true, speed: 2 });
    applyClock(b, state, 90_000);
    expect(b.playing).toBe(true);
    expect(b.speed).toBe(2);
    for (const dt of [0, 250, 4000]) expect(b.tickAt(90_000 + dt)).toBeCloseTo(a.tickAt(1500 + dt), 6);
  });

  test("pause and seek carry over", () => {
    const a = new ReplayClock(timeline()), b = new ReplayClock(timeline());
    b.play(0);
    a.seek(900, 0);
    applyClock(b, readClock(a, 10), 5000);
    expect(b.playing).toBe(false);
    expect(b.tickAt(9999)).toBe(900);
  });

  test("a state at the very end leaves the other clock stopped at the end, not started over", () => {
    const tl = timeline();
    const b = new ReplayClock(tl);
    applyClock(b, { tick: tl.endTick, playing: true, speed: 1 }, 0);
    expect(b.playing).toBe(false);
    expect(b.tickAt(1000)).toBe(tl.endTick);
  });

  test("a tick outside the recording is clamped, and a bad speed ignored", () => {
    const tl = timeline();
    const b = new ReplayClock(tl);
    applyClock(b, { tick: 1e9, playing: false, speed: 0 }, 0);
    expect(b.tickAt(0)).toBe(tl.endTick);
    expect(b.speed).toBe(1);
  });

  test("clocks that agree are left alone; a different tick, speed or play state is not", () => {
    const a = new ReplayClock(timeline());
    a.seek(300, 0);
    a.play(0);
    const s = readClock(a, 1000);
    expect(clockAgrees(a, s, 1000)).toBe(true);
    // A little drift while playing is fine; a seek is not.
    expect(clockAgrees(a, { ...s, tick: s.tick + 5 }, 1000)).toBe(true);
    expect(clockAgrees(a, { ...s, tick: s.tick + 90 }, 1000)).toBe(false);
    expect(clockAgrees(a, { ...s, playing: false }, 1000)).toBe(false);
    expect(clockAgrees(a, { ...s, speed: 4 }, 1000)).toBe(false);
    // Paused clocks must match to the tick.
    a.pause(1000);
    const p = readClock(a, 1000);
    expect(clockAgrees(a, p, 2000)).toBe(true);
    expect(clockAgrees(a, { ...p, tick: p.tick + 2 }, 2000)).toBe(false);
  });

  test("messages are checked before use", () => {
    expect(isSyncMessage({ kind: "hello", from: "a" })).toBe(true);
    expect(isSyncMessage({ kind: "state", from: "a", state: { tick: 1, playing: false, speed: 1 } })).toBe(true);
    expect(isSyncMessage({ kind: "state", from: "a", state: { tick: "1", playing: false, speed: 1 } })).toBe(false);
    expect(isSyncMessage({ kind: "state", from: "a" })).toBe(false);
    expect(isSyncMessage({ kind: "hello" })).toBe(false);
    expect(isSyncMessage(null)).toBe(false);
    expect(syncChannelName("abc")).toBe("h2live.replay.abc");
  });
});

describe("interpolation", () => {
  test("the replay sample between two 1 Hz frames is part-way between them", () => {
    const tl = timeline();
    const p = tl.sample(10 * TICKS_PER_SECOND + TICKS_PER_SECOND / 4)[0]!;
    expect(p.x).toBeCloseTo(20.5, 9);
  });

  const at = (x: number, extra: Partial<SampledPlayer> = {}): SampledPlayer => ({ index: 0, alive: true, x, y: 0, z: 0, yaw: 0, vehicle: null, ...extra });

  test("a new player appears where it is", () => {
    const shown = new Map<number, SmoothPlayer>();
    smoothPlayers(shown, [at(5)], 16);
    expect(shown.get(0)!.x).toBe(5);
  });

  test("shown players close on the sample without passing it, at the same pace whatever the frame rate", () => {
    const fast = new Map<number, SmoothPlayer>(), slow = new Map<number, SmoothPlayer>();
    smoothPlayers(fast, [at(0)], 16); smoothPlayers(slow, [at(0)], 16);
    for (let i = 0; i < 4; i++) smoothPlayers(fast, [at(1)], 8);
    smoothPlayers(slow, [at(1)], 32);
    expect(fast.get(0)!.x).toBeGreaterThan(0);
    expect(fast.get(0)!.x).toBeLessThan(1);
    expect(fast.get(0)!.x).toBeCloseTo(slow.get(0)!.x, 9);
    for (let i = 0; i < 100; i++) smoothPlayers(fast, [at(1)], 16);
    expect(fast.get(0)!.x).toBeCloseTo(1, 6);
  });

  test("a jump (respawn, teleport), a seek or a death moves at once", () => {
    const shown = new Map<number, SmoothPlayer>();
    smoothPlayers(shown, [at(0)], 16);
    smoothPlayers(shown, [at(40)], 16);
    expect(shown.get(0)!.x).toBe(40);
    smoothPlayers(shown, [at(41)], 16, { snap: true });
    expect(shown.get(0)!.x).toBe(41);
    smoothPlayers(shown, [at(42, { alive: false })], 16);
    expect(shown.get(0)).toMatchObject({ x: 42, alive: false });
  });

  test("yaw eases the short way round", () => {
    const shown = new Map<number, SmoothPlayer>();
    smoothPlayers(shown, [at(0, { yaw: 3.1 })], 16);
    smoothPlayers(shown, [at(0, { yaw: -3.1 })], 16);
    // Through π, not back through 0.
    expect(Math.abs(shown.get(0)!.yaw)).toBeGreaterThan(3.1);
  });

  test("players that leave the sample are dropped", () => {
    const shown = new Map<number, SmoothPlayer>();
    smoothPlayers(shown, [at(0), at(1, { index: 1 })], 16);
    smoothPlayers(shown, [at(1, { index: 1 })], 16);
    expect([...shown.keys()]).toEqual([1]);
  });
});

describe("camera", () => {
  const box = { min: [-40, -40, 0] as [number, number, number], max: [40, 40, 10] as [number, number, number] };

  test("the opening view is centred, outside the map, above it and to the south-east", () => {
    const f = cameraFit(box, 42, 16 / 9);
    expect(f.centre).toEqual([0, 0, 5]);
    expect(Math.hypot(...f.offset)).toBeGreaterThan(f.radius);
    expect(f.offset[0]).toBeGreaterThan(0);
    expect(f.offset[1]).toBeLessThan(0);
    expect(f.offset[2]).toBeGreaterThan(0);
  });

  test("the top-down height shows the whole floor, and a narrow window needs more", () => {
    const wide = cameraFit(box, 42, 2), tall = cameraFit(box, 42, 0.5);
    const half = Math.tan((42 * Math.PI) / 360);
    expect((wide.topDistance - 5) * half).toBeGreaterThanOrEqual(40);
    expect(tall.topDistance).toBeGreaterThan(wide.topDistance);
  });

  test("players are life size on a small map and bigger, to a limit, on a large one", () => {
    expect(markerScale({ min: [0, 0, 0], max: [20, 20, 5] })).toBe(1);
    expect(markerScale(box)).toBeCloseTo(Math.hypot(80, 80) / 45, 9);
    expect(markerScale({ min: [0, 0, 0], max: [900, 900, 5] })).toBe(4);
  });

  test("following starts close to the player whatever the size of the level", () => {
    // From far out on a huge campaign level: a few units from a life-size model, not a share of the map.
    expect(followDistance(900, 4, true, 2.4)).toBe(FOLLOW_DISTANCE);
    expect(followDistance(60, 1, true, 0.6)).toBe(FOLLOW_DISTANCE);
    // Already closer than that: stay there, but not inside what the controls allow.
    expect(followDistance(2, 1, true, 0.6)).toBe(2);
    expect(followDistance(0.5, 1, true, 0.6)).toBe(1.2);
    // A capsule is drawn bigger on a big map, and is followed from as much farther back.
    expect(followDistance(900, 4, false, 2.4)).toBe(FOLLOW_DISTANCE * 4);
    expect(followDistance(900, 1, false, 0.6)).toBe(FOLLOW_DISTANCE);
  });
});

describe("window placement", () => {
  const view = { w: 1280, h: 800 };

  test("opens in the lower right, inside the viewport", () => {
    const r = placeWindow(null, view);
    expect(r.x + r.w).toBeLessThanOrEqual(view.w);
    expect(r.y + r.h).toBeLessThanOrEqual(view.h);
    expect(r.x).toBeGreaterThan(view.w / 3);
  });

  test("a dragged window can't leave the screen or shrink below its minimum", () => {
    expect(placeWindow({ x: -500, y: 5000, w: 600, h: 400 }, view)).toEqual({ x: 8, y: 392, w: 600, h: 400 });
    expect(placeWindow({ x: 100, y: 100, w: 10, h: 10 }, view)).toMatchObject({ w: MIN_W, h: MIN_H });
  });

  test("a window bigger than a phone's screen is cut to fit", () => {
    const r = placeWindow({ x: 900, y: 300, w: 760, h: 520 }, { w: 390, h: 700 });
    expect(r).toEqual({ x: 8, y: 172, w: 374, h: 520 });
  });
});
