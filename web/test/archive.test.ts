import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { brotliDecompressSync } from "node:zlib";
import { decodeArchive } from "../src/api/archive";
import { PackError, isMachineBaseline } from "../src/api/pack";

const fixtures = join(import.meta.dir, "fixtures");
const header = [72, 50, 65, 82, 1];

describe("finished replay archive", () => {
  test("independently decodes a full Rust-produced multiplayer segment", () => {
    const bytes = brotliDecompressSync(readFileSync(join(fixtures, "archive/mp16.br")));
    const frames = decodeArchive(bytes);
    const golden = JSON.parse(readFileSync(join(fixtures, "pack/v7-multiplayer.json"), "utf8"));
    expect(frames.slice(0, golden.length)).toEqual(golden);
    expect(frames).toHaveLength(450);
    expect(frames.at(-1)?.tick).toBe(9898);
    expect(() => decodeArchive(bytes.subarray(0, -1))).toThrow(PackError);
    expect(() => decodeArchive(Buffer.concat([bytes, Buffer.from([0])]))).toThrow(PackError);
  });

  test("preserves unsigned 64-bit ticks and the sign bit of raw binary64 values", () => {
    const key = (s: string) => [s.length, ...new TextEncoder().encode(s), 0];
    const bytes = Uint8Array.from([...header, 1, 0, 4,
      ...key("game_id"), 2, 1, 4, 0xef, 0xbb, 0xbf, 103,
      ...key("players"), 1, 0, 0,
      ...key("tick"), 4, 0, 0, 1, // unsigned reinterpretation of -1
      ...key("x"), 5, 0, 0, ...Array(9).fill(255), 1, // binary64 negative zero
    ]);
    const [frame] = decodeArchive(bytes);
    expect(frame?.tick).toBe(0xffffffffffffffffn);
    expect(frame?.game_id).toBe("\uFEFFg");
    expect(Object.is(Reflect.get(frame!, "x"), -0)).toBe(true);
  });

  test("rejects unsupported versions, oversized segments and overlong integers", () => {
    for (const bytes of [[...header.slice(0, 4), 2], [...header, 0x85, 7],
      [...header, ...Array(9).fill(255), 2]]) {
      expect(() => decodeArchive(Uint8Array.from(bytes))).toThrow(PackError);
    }
  });
});

import { ArchiveWindow } from "../src/viewer/archive";
import { chunkIndex } from "../src/api/chunk";
import { ReplayTimeline } from "../src/state/replay";

test("a direct seek opens only its target segment through a checkpoint rewind", async () => {
  const game = "7f3c0b5e2a9d4c1f8e6b3a2d1c0f9e8d";
  const meta = {from_tick:9000, to_tick:9898, frames:450, bytes:15556, sha256:"b".repeat(64), keyframes:[0]};
  const index = chunkIndex({game_id:game, archive_sha256:"a".repeat(64), ended:true, items:[meta, meta, meta]})!;
  const calls: number[] = [];
  const window = new ArchiveWindow(index, async c => {
    if (c.codec !== "archive") throw new Error("expected published archive");
    calls.push(c.ordinal);
    return brotliDecompressSync(readFileSync(join(fixtures, "archive/mp16.br")));
  });
  await window.at(10900);
  expect(calls).toEqual([2]);
  expect(window.frames[0]?.tick).toBe(10800);
  expect(isMachineBaseline(window.frames[0]!)).toBe(true);
  expect(isMachineBaseline(window.frames[1]!)).toBe(false);
  const timeline = new ReplayTimeline(window.frames, [], window.segments);
  expect([timeline.startTick, timeline.endTick]).toEqual([9000,11698]);
  expect(timeline.cuts).toEqual([9900,10800]);
  expect(timeline.tickFromLatestRun(9100)).toBe(10900);
  await window.at(9000);
  expect(calls).toEqual([2,0]);
  expect(window.frames).toHaveLength(450);
});

test("a looping clip retains its opening segment across a distant seek", async () => {
  const game = "7f3c0b5e2a9d4c1f8e6b3a2d1c0f9e8d";
  const meta = { from_tick: 9000, to_tick: 9898, frames: 450, bytes: 15556, sha256: "b".repeat(64), keyframes: [0] };
  const index = chunkIndex({ game_id: game, archive_sha256: "a".repeat(64), ended: true, items: [meta, meta, meta] })!;
  const calls: number[] = [];
  const window = new ArchiveWindow(index, async (c) => {
    if (c.codec !== "archive") throw new Error("expected published archive");
    calls.push(c.ordinal);
    return brotliDecompressSync(readFileSync(join(fixtures, "archive/mp16.br")));
  }, 9300);
  await window.at(9300);
  await window.at(11000);
  expect(window.frames[0]?.tick).toBe(10800);
  await window.at(9300);
  expect(calls).toEqual([0, 2]);
  expect(window.ready(9300)).toBe(true);
  expect(window.frames[0]?.tick).toBe(9000);
});

test("wide source ticks are projected into a precise local playback interval", () => {
  const index = chunkIndex({game_id:"g", ended:true, archive_sha256:"0".repeat(64), items:[{
    from_tick:"18446744073709551500", to_tick:"18446744073709551615", frames:2,
    bytes:1, sha256:"1".repeat(64), keyframes:[],
  }]});
  expect(index?.items.map(c => [c.from_tick, c.to_tick])).toEqual([[0,115]]);
});

test("a direct segment seek restores known machine positions before applying sparse samples", async () => {
  const game = "7f3c0b5e2a9d4c1f8e6b3a2d1c0f9e8d";
  const segment = { from_tick: 9000, to_tick: 9898, frames: 450, bytes: 15556,
    sha256: "b".repeat(64), keyframes: [1], machine_entry: [{i: 3, p: 0}, {i: 7, p: 128}] };
  const previous = { ...segment, from_tick: 8980, to_tick: 8998, frames: 10, keyframes: [0], machine_entry: [] };
  const index = chunkIndex({game_id: game, archive_sha256: "a".repeat(64), ended: true, items: [previous, segment]})!;
  const calls: number[] = [];
  const window = new ArchiveWindow(index, async c => {
    if (c.codec !== "archive") throw new Error("expected archive");
    calls.push(c.ordinal);
    return brotliDecompressSync(readFileSync(join(fixtures, "archive/mp16.br")));
  });
  await window.at(9000);
  expect(calls).toEqual([1]);
  const timeline = new ReplayTimeline(window.frames, [], window.segments);
  expect(timeline.sampleMachinePositions(9000)?.get(3)?.from).toBe(0);
  expect(timeline.sampleMachinePositions(9000)?.get(7)?.from).toBe(128 / 255);
  expect(timeline.sampleMachinePositions(9002)).toBeUndefined();
  expect(isMachineBaseline(timeline.frames[1]!)).toBe(true);
  expect(window.frames[0]!.machine_positions).toBeUndefined();
});

test("machine travel holds until a rewound segment and follows contiguous wide ticks", () => {
  const base = {frames:1, bytes:1, sha256:"b".repeat(64), keyframes:[]};
  const wide = 1n << 60n;
  const items = [wide, wide + 2n, wide].map(t => ({
    ...base, from_tick:String(t), to_tick:String(t),
  }));
  const index = chunkIndex({game_id:"g", archive_sha256:"a".repeat(64), ended:true, items})!;
  const window = new ArchiveWindow(index, async () => { throw new Error("unexpected fetch"); });
  const timeline = new ReplayTimeline([
    {game_id:"g", tick:0, players:[], machine_positions:[{i:1,p:0},{i:2,p:64}]},
    {game_id:"g", tick:2, players:[]},
    {game_id:"g", tick:4, players:[], machine_positions:[{i:1,p:255}]},
  ], [], window.segments);
  expect(timeline.cuts).toEqual([4]);
  expect(timeline.sampleMachinePositions(3)?.get(1)).toEqual({from:0,to:0,mix:0});
  expect(timeline.sampleMachinePositions(3)?.get(2)?.from).toBe(64/255);
  expect(timeline.sampleMachinePositions(4)?.get(1)?.from).toBe(1);
  expect(timeline.sampleMachinePositions(4)?.has(2)).toBe(false);
});
