import { describe, expect, test } from "bun:test";
import type { Frame, WireEvent } from "../src/api/types";
import { ReplayClock, ReplayTimeline, fetchRecording } from "../src/state/replay";
import { FrameBuffer } from "../src/state/frames";
import { ev, resetPos } from "./helpers";

const frame = (tick: number, x: number, alive = true): Frame => ({
  game_id: "g", tick, players: [{ index: 0, alive, pos: [x, 0, 1], yaw: 0 }, { index: 1, alive: true, pos: [0, x, 1], yaw: 0 }],
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
  return new ReplayTimeline(frames, events);
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

  test("the game's GameFrame events stand in for fetched frames they cover", () => {
    resetPos();
    const t = new ReplayTimeline([frame(0, 0)], [ev("GameFrame", "game-g", frame(0, 1)), ev("GameFrame", "game-g", frame(30, 3))]);
    expect(t.frames.map((f) => f.tick)).toEqual([0, 30]);
    expect(t.frames[0]!.players[0]!.pos[0]).toBe(1);
    // Fewer GameFrame events than fetched frames (a log read cut short):
    // the fetched frames.
    resetPos();
    const u = new ReplayTimeline([frame(0, 0), frame(30, 3), frame(60, 6)], [ev("GameFrame", "game-g", frame(0, 1))]);
    expect(u.frames.map((f) => f.players[0]!.pos[0])).toEqual([0, 3, 6]);
  });
});

/** A campaign recording from before the game clock kept counting through
 *  reverts (docs/live/replay-fidelity.md): the player walks along x from
 *  100 to 130, the game reverts to a checkpoint (tick 300 again) and they
 *  walk from 500, then the mission restarts (tick 4) at -50. */
function rewound() {
  resetPos();
  const frames: Frame[] = [];
  const events: WireEvent[] = [ev("GameStarted", "game-g", { tick: 0 })];
  const at = (tick: number, x: number) => {
    const f = frame(tick, x);
    frames.push(f);
    events.push(ev("GameFrame", "game-g", f));
  };
  for (let i = 0; i <= 3; i++) at(300 + 30 * i, 100 + 10 * i);
  events.push(ev("PlayerKilled", "game-g", { tick: 391, victim: 0, kind: "world" }));
  // The revert's first second: a kill before its first recorded frame.
  events.push(ev("PlayerKilled", "game-g", { tick: 305, victim: 0, kind: "world", pos: [501, 0, 0] }));
  for (let i = 0; i <= 3; i++) at(310 + 30 * i, 500 + 10 * i);
  for (let i = 0; i <= 3; i++) at(4 + 30 * i, -50 - 10 * i);
  events.push(ev("GameEnded", "game-g", { tick: 100, reason: "lost" }));
  return { frames, events };
}

describe("a recording whose ticks run backwards (campaign reverts)", () => {
  const xs = (t: ReplayTimeline) => t.frames.map((f) => f.players[0]!.pos[0]);

  for (const from of ["events", "fetched frames"] as const) {
    test(`plays its runs one after another, from the ${from}`, () => {
      const { frames, events } = rewound();
      const t = new ReplayTimeline(frames, from === "events" ? events : events.filter((e) => e.type !== "GameFrame"));
      expect(xs(t)).toEqual([100, 110, 120, 130, 500, 510, 520, 530, -50, -60, -70, -80]);
      const ticks = t.frames.map((f) => f.tick);
      for (let i = 1; i < ticks.length; i++) expect(ticks[i]!).toBeGreaterThan(ticks[i - 1]!);
      expect(t.startTick).toBe(300);
      expect(t.endTick).toBe(ticks[ticks.length - 1]! + 6);
      // A cut starts each later run: the second's at its first frame or
      // the kill just before it.
      expect(t.cuts).toHaveLength(2);
      expect(t.cuts[0]!).toBeGreaterThan(ticks[3]!);
      expect(t.cuts[0]!).toBeLessThanOrEqual(ticks[4]!);
      expect(t.cuts[1]).toBe(ticks[8]!);
    });
  }

  test("never flicks between runs while playing", () => {
    const { frames, events } = rewound();
    const t = new ReplayTimeline(frames, events);
    let last = t.sample(t.startTick)[0]!.x, flips = 0;
    for (let tick = t.startTick; tick <= t.endTick; tick += 1) {
      const x = t.sample(tick)[0]!.x;
      if (Math.abs(x - last) > 50) flips++;
      last = x;
    }
    expect(flips).toBe(2);
  });

  test("events land in the run they happened in", () => {
    const { frames, events } = rewound();
    const t = new ReplayTimeline(frames, events);
    const [first, second] = t.kills;
    // Between the first run's last frame and the second's first.
    expect(first!.tick).toBe(391);
    expect(second!.tick).toBeGreaterThan(t.frames[3]!.tick);
    expect(second!.tick).toBeLessThan(t.frames[4]!.tick);
    expect(second!.at).toEqual({ x: 501, y: 0, z: 0 });
    // The first kill's place comes from its own run's frames.
    expect(first!.at!.x).toBeCloseTo(130);
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

describe("kill positions", () => {
  test("PlayerKilled.pos wins over the frames", () => {
    resetPos();
    const t = new ReplayTimeline([frame(0, 0), frame(30, 3)], [ev("PlayerKilled", "game-g", { tick: 15, victim: 0, kind: "world", pos: [7, 8, 9] })]);
    expect(t.kills[0]!.at).toEqual({ x: 7, y: 8, z: 9 });
  });
});

describe("players sent at the origin (older senders, nowhere known)", () => {
  test("are not drawn there: they hold where they were, or wait to appear", () => {
    resetPos();
    const origin = (tick: number, alive: boolean): Frame => ({
      game_id: "g", tick, players: [{ index: 0, alive, pos: [0, 0, 0], yaw: 0 }, { index: 1, alive: true, pos: [0, 40, 0], yaw: 0 }],
    });
    const t = new ReplayTimeline([frame(0, 40), origin(30, true), frame(60, 42), origin(90, false)], []);
    for (let tick = 0; tick <= 90; tick += 3) {
      const p = t.sample(tick).find((q) => q.index === 0);
      if (p) expect(Math.hypot(p.x, p.y, p.z)).toBeGreaterThan(30);
    }
    expect(t.sample(90).find((q) => q.index === 1)!.y).toBe(40);
  });

  test("and the live buffer leaves them out too", () => {
    const b = new FrameBuffer();
    b.push({ game_id: "g", tick: 0, players: [{ index: 0, alive: false, pos: [0, 0, 0], yaw: 0 }, { index: 1, alive: true, pos: [5, 5, 5], yaw: 1 }] }, 0);
    expect(b.frames[0]!.players.map((p) => p.index)).toEqual([1]);
    expect(b.bounds.minX).toBe(5);
  });
});

describe("the transport's marks, steps and killcam", () => {
  function marked() {
    resetPos();
    const frames: Frame[] = [frame(0, 0), frame(30, 3), frame(60, 6), frame(90, 9), frame(120, 12)];
    frames[0]!.bsp = 0; frames[1]!.bsp = 0; frames[2]!.bsp = 1; frames[3]!.bsp = 1; frames[4]!.bsp = 1;
    const events = [
      ev("PlayerKilled", "game-g", { tick: 40, victim: 0, killer: 1, kind: "kill" }),
      ev("PlayerKilled", "game-g", { tick: 100, victim: 1, kind: "world" }),
      ev("PlayerKilled", "game-g", { tick: 110, victim: 1, killer: 1, kind: "suicide" }),
    ];
    return new ReplayTimeline(frames, events);
  }

  test("kills, deaths and zone changes, in order", () => {
    const t = marked();
    expect(t.marks.map((m) => [m.tick, m.kind])).toEqual([[40, "kill"], [60, "zone"], [100, "death"], [110, "death"]]);
    expect(t.marks[1]!.bsp).toBe(1);
  });

  test("cuts are marks", () => {
    const { frames, events } = rewound();
    const t = new ReplayTimeline(frames, events);
    expect(t.marks.filter((m) => m.kind === "cut").map((m) => m.tick)).toEqual(t.cuts);
  });

  test("frame stepping goes from recorded frame to recorded frame", () => {
    const t = marked();
    expect(t.frameStep(0, 1)).toBe(30);
    expect(t.frameStep(45, 1)).toBe(60);
    expect(t.frameStep(45, -1)).toBe(30);
    expect(t.frameStep(30, -1)).toBe(0);
    expect(t.frameStep(0, -1)).toBe(0);
    expect(t.frameStep(120, 1)).toBe(t.endTick);
  });

  test("mark stepping, and back from just after a mark skips it", () => {
    const t = marked();
    expect(t.markStep(0, 1)).toBe(40);
    expect(t.markStep(40, 1)).toBe(60);
    expect(t.markStep(110, 1)).toBeNull();
    expect(t.markStep(130, -1)).toBe(110);
    expect(t.markStep(116, -1)).toBe(100);
    expect(t.markStep(101, -1)).toBe(60);
    expect(t.markStep(40, -1)).toBeNull();
  });

  test("a killcam starts five seconds before, slowed, through the killer's eyes", () => {
    const t = marked();
    expect(t.killcam(t.kills[0]!)).toEqual({ from: 0, speed: 0.5, follow: 1 });
    expect(t.killcam(t.kills[1]!)).toEqual({ from: 0, speed: 0.5, follow: 1 });
    const late = { ...t.kills[0]!, tick: 115 };
    expect(t.killcam(late).from).toBe(0);
    expect(t.killcam({ ...late, tick: 1000 }).from).toBe(t.endTick);
  });
});

describe("fetchRecording", () => {
  test("reads the frames once: the events come without their GameFrames", async () => {
    const calls: string[] = [];
    const a = {
      frames: async (id: string) => { calls.push("frames " + id); return [frame(0, 1)]; },
      streamEvents: async (stream: string, _s?: AbortSignal, opts?: { exclude?: string }) => { calls.push(`events ${stream} ${opts?.exclude}`); return []; },
    };
    const r = await fetchRecording(a, "g");
    expect(calls.sort()).toEqual(["events game-g GameFrame", "frames g"]);
    expect(r.frames).toHaveLength(1);
  });

  test("a failed event read still replays the frames", async () => {
    const a = {
      frames: async () => [frame(0, 1)],
      streamEvents: async () => { throw new Error("nope"); },
    };
    expect((await fetchRecording(a, "g")).events).toBeNull();
  });
});

describe("killNear", () => {
  test("the latest kill at or just after the playhead, else the next", () => {
    resetPos();
    const t = new ReplayTimeline([frame(0, 0), frame(300, 3)], [
      ev("PlayerKilled", "game-g", { tick: 100, victim: 0, killer: 1, kind: "kill" }),
      ev("PlayerKilled", "game-g", { tick: 200, victim: 1, killer: 0, kind: "kill" }),
    ]);
    expect(t.killNear(0)!.tick).toBe(100);
    expect(t.killNear(150)!.tick).toBe(100);
    expect(t.killNear(180)!.tick).toBe(200);
    expect(t.killNear(290)!.tick).toBe(200);
    expect(new ReplayTimeline([frame(0, 0)], []).killNear(0)).toBeNull();
  });
});

describe("cutscenes (cinematic frames)", () => {
  const cut = (tick: number): Frame => ({ game_id: "g", tick, players: [], bsp: 1, cinematic: true });

  /** Play at 0..60, a cutscene at 90..150 (the scripts move the player from
   *  x 6 to x 50 meanwhile), play again from 180. */
  function withCutscene(end = 240) {
    resetPos();
    const frames = [frame(0, 0), frame(30, 3), frame(60, 6), cut(90), cut(120), cut(150), frame(180, 50), frame(210, 51)];
    return new ReplayTimeline(frames, [ev("GameEnded", "game-g", { tick: end, reason: "finished" })]);
  }

  test("the timeline knows each cutscene and marks it on the scrubber", () => {
    const t = withCutscene();
    expect(t.cinematics).toEqual([{ from: 90, to: 180 }]);
    expect(t.marks.filter((m) => m.kind === "cinematic")).toEqual([{ tick: 90, kind: "cinematic", to: 180 }]);
    expect(t.cinematicSpanAt(89)).toBeNull();
    expect(t.cinematicSpanAt(90)).toEqual({ from: 90, to: 180 });
    expect(t.cinematicSpanAt(179)).toEqual({ from: 90, to: 180 });
    expect(t.cinematicSpanAt(180)).toBeNull();
    expect(t.cinematicAt(100)).toBe(true);
    expect(t.cinematicAt(185)).toBe(false);
  });

  test("nothing stale or teleported is drawn into or out of one", () => {
    const t = withCutscene();
    for (let tick = 90; tick < 180; tick += 5) {
      expect(t.sample(tick)).toEqual([]);
      expect(t.sampleActors(tick)).toEqual([]);
      expect(t.sampleVehicles(tick)).toEqual([]);
    }
    // Before it, players hold where they were rather than slide off
    // towards the cutscene; after it, they are where the scripts left them.
    expect(t.sample(80).find((p) => p.index === 0)!.x).toBe(6);
    expect(t.sample(180).find((p) => p.index === 0)!.x).toBe(50);
    expect(t.sample(195).find((p) => p.index === 0)!.x).toBeCloseTo(50.5);
  });

  test("playing skips it; seeking into one shows it", () => {
    const t = withCutscene();
    const c = new ReplayClock(t);
    c.seek(60, 0);
    c.play(0);
    // One second in: tick 90, the cutscene's start, so on to its end.
    expect(c.tickAt(1000)).toBe(180);
    expect(c.playing).toBe(true);
    expect(c.tickAt(2000)).toBeCloseTo(210);
    c.pause(2000);
    c.seek(120, 2000);
    expect(c.tickAt(5000)).toBe(120);
    expect(t.cinematicAt(c.tickAt(5000))).toBe(true);
  });

  test("playback stops at the end when the last stretch is a cutscene", () => {
    resetPos();
    const t = new ReplayTimeline([frame(0, 0), frame(30, 3), cut(60), cut(90)], [ev("GameEnded", "game-g", { tick: 100, reason: "finished" })]);
    expect(t.cinematics).toEqual([{ from: 60, to: 100 }]);
    expect(t.cinematicSpanAt(100)).toEqual({ from: 60, to: 100 });
    const c = new ReplayClock(t);
    c.seek(30, 0);
    c.play(0);
    expect(c.tickAt(1000)).toBe(100);
    expect(c.playing).toBe(false);
  });

  test("the live buffer shows a cutscene and no one in it", () => {
    const b = new FrameBuffer(48, 0);
    b.push(frame(0, 1), 0);
    b.push(frame(2, 1), 67);
    b.push(cut(4), 133);
    b.push(cut(6), 200);
    expect(b.cinematic(200)).toBe(true);
    expect(b.sample(200)).toEqual([]);
    b.push(frame(8, 30), 267);
    expect(b.cinematic(267)).toBe(false);
    expect(b.sample(267).find((p) => p.index === 0)!.x).toBe(30);
  });
});
