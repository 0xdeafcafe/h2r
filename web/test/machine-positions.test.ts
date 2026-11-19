import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { FrameDecoder, markMachineBaseline, records } from "../src/api/pack";
import type { Frame } from "../src/api/types";
import { FrameBuffer, TICKS_PER_SECOND } from "../src/state/frames";
import { ReplayTimeline } from "../src/state/replay";
import { replayFrame } from "../src/viewer/session";

function frame(tick: number, over: Partial<Frame> = {}): Frame {
  return { game_id: "g", tick, players: [], ...over };
}

test("live travel interpolates between the surrounding frames and holds through quiet frames", () => {
  const buf = new FrameBuffer(4, 0);
  const now = (tick: number) => tick * 1000 / TICKS_PER_SECOND;
  buf.push(frame(10, { machine_positions: [{ i: 3, p: 0 }, { i: 7, p: 64 }] }), now(10));
  buf.push(frame(12), now(12));
  buf.push(frame(14, { machine_positions: [{ i: 3, p: 255 }, { i: 5, p: 128 }] }), now(14));
  expect(buf.sampleMachinePositions(now(9))).toBeUndefined();
  expect(buf.sampleMachinePositions(now(11))?.get(3)).toEqual({ from: 0, to: 0, mix: 0 });
  const samples = buf.sampleMachinePositions(now(13))!;
  expect(samples.get(3)).toEqual({ from: 0, to: 1, mix: 0.5 });
  expect(samples.get(5)).toBeUndefined();
  for (let tick = 16; tick <= 30; tick += 2) buf.push(frame(tick), now(tick));
  expect(buf.sampleMachinePositions(now(30))?.get(3)?.from).toBe(1);
  expect(buf.sampleMachinePositions(now(30))?.get(7)?.from).toBe(64 / 255);
});

test("a replay can seek across sparse travel, with no interpolation across a cut", () => {
  const frames = [
    frame(10, { machine_positions: [{ i: 3, p: 240 }, { i: 7, p: 128 }] }),
    frame(12),
    frame(14, { machine_positions: [{ i: 3, p: 16 }] }),
    frame(16),
    frame(18, { cut: 4, machine_positions: [{ i: 3, p: 0 }] }),
    ...Array.from({ length: 10 }, (_, i) => frame(20 + i * 2)),
  ];
  const tl = new ReplayTimeline(frames, []);
  const at = (tick: number) => replayFrame(tl, tick, new Map()).machinePositions;
  // The viewer receives both endpoints and can blend through the wrap.
  expect(at(13)?.get(3)).toEqual({ from: 240 / 255, to: 16 / 255, mix: 0.5 });
  expect(at(38)?.get(3)).toEqual({ from: 0, to: 0, mix: 0 });
  expect(at(38)?.has(7)).toBe(false);
  expect(at(17)?.get(3)).toEqual({ from: 16 / 255, to: 16 / 255, mix: 0 });
  expect(at(17)?.get(7)?.from).toBe(128 / 255);
  expect(new ReplayTimeline([frame(10)], []).sampleMachinePositions(10)).toBeUndefined();
});

test("a map with 195 moving parts fits in a single travel baseline", () => {
  const buf = new FrameBuffer(4, 0);
  buf.push(frame(10, {
    machine_positions: Array.from({ length: 195 }, (_, i) => ({ i, p: 128 })),
  }), 0);
  expect(buf.sampleMachinePositions(1)?.size).toBe(195);
});

test("packed v7 baselines clear unknown devices in joined and seeked playback", () => {
  const bin = new Uint8Array(readFileSync(new URL("./fixtures/pack/v7-machine-positions.bin", import.meta.url)));
  const all: Frame[] = [];
  new FrameDecoder().forEachRecord(bin, f => all.push(f));
  const timeline = new ReplayTimeline(all, []);
  const at = (t: number) => timeline.sampleMachinePositions(t);
  expect(at(11)).toBeUndefined();
  expect(at(13)?.get(7)).toEqual({ from: 64 / 255, to: 128 / 255, mix: 0.5 });
  expect(at(14)?.get(23)?.from).toBe(200 / 255);
  expect(at(16)).toBeUndefined();
  expect(at(19)?.get(7)?.from).toBe(1);
  expect(at(20)?.has(7)).toBe(false);
  expect(at(23)?.get(23)).toEqual({ from: 40 / 255, to: 168 / 255, mix: 0.5 });
  expect(at(14)?.get(23)?.from).toBe(200 / 255);

  const joined = new FrameBuffer(4, 0);
  const decoder = new FrameDecoder();
  for (const record of records(bin).slice(5)) {
    const f = decoder.decode(record).frame;
    joined.push(f, f.tick * 1000 / TICKS_PER_SECOND);
  }
  const t23 = 23 * 1000 / TICKS_PER_SECOND;
  expect(joined.sampleMachinePositions(t23)).toEqual(at(23));
});

test("archive metadata restores an empty baseline without changing sparse JSON", () => {
  const initial = frame(10, { machine_positions: [{ i: 4, p: 255 }] });
  const empty = frame(12);
  markMachineBaseline(empty);
  const tl = new ReplayTimeline([initial, empty], []);
  expect(JSON.stringify(empty)).toBe('{"game_id":"g","tick":12,"players":[]}');
  expect(tl.sampleMachinePositions(11)?.get(4)?.from).toBe(1);
  expect(tl.sampleMachinePositions(12)).toBeUndefined();
});
