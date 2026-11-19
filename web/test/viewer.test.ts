import { describe, expect, test } from "bun:test";
import type { Frame } from "../src/api/types";
import { ReplayClock, ReplayTimeline } from "../src/state/replay";
import { TICKS_PER_SECOND, type SampledPlayer } from "../src/state/frames";
import { applyClock, clockAgrees, isSyncMessage, nextCamera, readClock, smoothPlayers, syncChannelName, type SmoothPlayer } from "../src/viewer/sync";
import { EYE_HEIGHT, FOLLOW_DISTANCE, cameraFit, eyeView, followDistance, markerScale } from "../src/viewer/camera";
import { ViewerPlayback } from "../src/viewer/playback";

function frames(seconds: number): Frame[] {
  return Array.from({ length: seconds + 1 }, (_, s) => ({
    game_id: "g", tick: s * TICKS_PER_SECOND,
    players: [{ index: 0, alive: true, pos: [s * 2, 0, 0] as [number, number, number], yaw: 0 }],
  }));
}
const timeline = () => new ReplayTimeline(frames(60), []);

describe("viewer playback", () => {
  test("the live session survives game state updates and adopts the finished recording in place", async () => {
    const run = Bun.spawn([process.execPath, "--conditions=browser", `${import.meta.dir}/fixtures/viewer-session.ts`], {
      stdout: "ignore", stderr: "pipe",
    });
    const [exitCode, stderr] = await Promise.all([run.exited, new Response(run.stderr).text()]);
    if (exitCode !== 0) throw new Error(stderr);
  });

  test("a finished game opens at time zero and autoplays from its first frame", () => {
    const recording = new ReplayTimeline(frames(60).slice(10), []);
    const playback = new ViewerPlayback(false);
    playback.openRecording(recording, 1000);
    expect(playback.tickAt(1000, 0) - recording.startTick).toBe(0);
    expect(playback.playing).toBe(true);
    expect(playback.tickAt(2000, 0)).toBe(recording.startTick + TICKS_PER_SECOND);
  });

  test("a time link opens playing at that many seconds into the recording", () => {
    const recording = new ReplayTimeline(frames(60).slice(10), []);
    const playback = new ViewerPlayback(false);
    playback.openRecording(recording, 1000, "12.5");
    expect(playback.tickAt(1000, 0)).toBe(recording.startTick + 12.5 * TICKS_PER_SECOND);
    expect(playback.playing).toBe(true);
    expect(playback.tickAt(2000, 0)).toBe(recording.startTick + 13.5 * TICKS_PER_SECOND);
  });

  test("an invalid time opens at the start and a time past the end stays at the end", () => {
    const recording = timeline();
    for (const [query, tick, playing] of [
      ["bad", recording.startTick, true],
      ["-3", recording.startTick, true],
      ["100", recording.endTick, false],
    ] as const) {
      const playback = new ViewerPlayback(false);
      playback.openRecording(recording, 1000, query);
      expect(playback.tickAt(1000, 0)).toBe(tick);
      expect(playback.playing).toBe(playing);
    }
  });

  test("live follows arriving frames, scrubbing back holds the position, and Live returns to the edge", () => {
    const playback = new ViewerPlayback(true);
    const liveTimeline = (seconds: number) => new ReplayTimeline(frames(seconds), []);
    playback.updateLive(liveTimeline(10), 1000);
    expect(playback.followingLive).toBe(true);
    expect(playback.tickAt(1000, 290)).toBe(290);
    playback.updateLive(liveTimeline(11), 2000);
    expect(playback.tickAt(2000, 320)).toBe(320);

    playback.seek(210, 2000);
    playback.updateLive(liveTimeline(12), 3000);
    expect(playback.followingLive).toBe(false);
    expect(playback.playing).toBe(false);
    expect(playback.tickAt(3000, 350)).toBe(210);

    playback.goLive(3000);
    expect(playback.followingLive).toBe(true);
    expect(playback.tickAt(3000, 350)).toBe(350);
  });

  test("a match ending at the live edge carries the current position and play state into the replay", () => {
    const playback = new ViewerPlayback(true);
    playback.updateLive(new ReplayTimeline(frames(10), []), 1000);
    playback.endLive(1000, 290);
    expect(playback.followingLive).toBe(false);
    expect(playback.tickAt(1000, 0)).toBe(290);
    expect(playback.playing).toBe(true);

    // The buffer can run out while the recording is still loading.
    expect(playback.tickAt(2000, 0)).toBe(300);
    playback.openRecording(timeline(), 4000);
    expect(playback.tickAt(4000, 0)).toBe(300);
    expect(playback.playing).toBe(true);
    expect(playback.tickAt(5000, 0)).toBeCloseTo(330);
  });

  test("ending while paused in the buffer preserves the position through a campaign clock reset", () => {
    const playback = new ViewerPlayback(true);
    playback.updateLive(new ReplayTimeline(frames(10), []), 1000);
    playback.seek(210, 1000);
    playback.endLive(2000, 300);

    const recording = new ReplayTimeline([...frames(60), ...frames(12)], []);
    playback.openRecording(recording, 5000);
    expect(playback.tickAt(5000, 0)).toBe(recording.cuts[0]! + 210);
    expect(playback.playing).toBe(false);
  });
});

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

describe("first-person camera", () => {
  test("eyes over the feet, looking along the aim", () => {
    const v = eyeView({ x: 1, y: 2, z: 3, yaw: 0, aim: [0, 0.6, 0.8] }, 0.8);
    expect(v.eye[0]).toBe(1);
    expect(v.eye[2]).toBeCloseTo(3 + 0.72);
    expect(v.look[1] - v.eye[1]).toBeCloseTo(0.6);
    expect(v.look[2] - v.eye[2]).toBeCloseTo(0.8);
  });

  test("lower when crouched; level along the yaw without an aim; a default height without a model", () => {
    const stand = eyeView({ x: 0, y: 0, z: 0, yaw: Math.PI / 2 }, null);
    expect(stand.eye[2]).toBeCloseTo(EYE_HEIGHT);
    expect(stand.look[1]).toBeCloseTo(1);
    expect(stand.look[2]).toBeCloseTo(stand.eye[2]);
    const crouch = eyeView({ x: 0, y: 0, z: 0, yaw: 0, stance: "crouch" }, null);
    expect(crouch.eye[2]).toBeLessThan(stand.eye[2]);
    expect(eyeView({ x: 0, y: 0, z: 0, yaw: 0, aim: [0, 0, 0] }, 1).look[0]).toBeCloseTo(1);
  });
});

describe("camera requests between windows", () => {
  test("count up and travel as sync messages", () => {
    const a = nextCamera(null, "eyes", 3), b = nextCamera(a, "eyes", 3);
    expect(b.seq).toBe(a.seq + 1);
    expect(isSyncMessage({ kind: "camera", from: "w", camera: b })).toBe(true);
    expect(isSyncMessage({ kind: "camera", from: "w", camera: { ...b, mode: "drone" } })).toBe(false);
    expect(isSyncMessage({ kind: "camera", from: "w", camera: { ...b, follow: 1.5 } })).toBe(false);
  });
});

describe("a camera request's freshness", () => {
  test("a view that opens for a killcam takes it; one opened much later does not", async () => {
    const { cameraFresh } = await import("../src/viewer/sync");
    const c = nextCamera(null, "eyes", 2, 10_000);
    expect(cameraFresh(c, 10_500)).toBe(true);
    expect(cameraFresh(c, 20_000)).toBe(false);
    expect(cameraFresh(null, 10_000)).toBe(false);
  });
});
