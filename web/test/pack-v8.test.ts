import { expect, test } from "bun:test";
import { FrameDecoder, isMachineBaseline, PackError } from "../src/api/pack";

// An empty v7 key for game g, tick 10, wrapped as v8 stream 129.
// The two independent range packets change BSP, cinematic and machine 23.
const inner = Buffer.from("778039ab000028499204", "hex");
const key = Buffer.concat([Buffer.from([0xf8, 122, 0]), inner]);
const first = Buffer.from("e87a010100d9dcc9fa07f3b2c67b651c0000", "hex");
const second = Buffer.from("e87a020100d64de4aeab60808bb206000000", "hex");

test("v8 metadata, explicit zero travel, and persistent model state", () => {
  const d = new FrameDecoder();
  const k = d.decode(key);
  expect([k.sid, k.seq, k.keyframe, isMachineBaseline(k.frame)]).toEqual([129, 0, true, true]);
  expect(k.frame).toEqual({ game_id: "g", tick: 10, players: [] });
  const a = d.decode(first).frame;
  expect(a).toEqual({ game_id: "g", tick: 11, bsp: 0, cinematic: true, players: [], machine_positions: [{ i: 23, p: 0 }] });
  expect(isMachineBaseline(a)).toBe(false);
  expect(d.decode(second).frame).toEqual({ game_id: "g", tick: 12, bsp: 1, players: [], machine_positions: [{ i: 23, p: 128 }] });
});

test("a failed v8 record invalidates only its stream until a new key", () => {
  const d = new FrameDecoder();
  d.decode(key);
  const otherKey = Buffer.from(key);
  otherKey[1] = 1;
  d.decode(otherKey);
  expect(() => d.decode(first.subarray(0, -1))).toThrow(PackError);
  expect(() => d.decode(first)).toThrow("preceding frame");
  const other = Buffer.from(first);
  other[1] = 1;
  expect(d.decode(other).frame.tick).toBe(11);
  d.decode(key);
  expect(d.decode(first).frame.tick).toBe(11);
  d.forget(129);
  expect(() => d.decode(second)).toThrow("preceding frame");
});

test("v7 and v8 keys supersede one another on the same stream id", () => {
  const d = new FrameDecoder();
  const zeroKey = Buffer.concat([Buffer.from([24, 0]), inner]);
  const zeroDelta = Buffer.concat([Buffer.from([8]), first.subarray(2)]);
  d.decode(zeroKey);
  d.decode(inner);
  expect(() => d.decode(zeroDelta)).toThrow("preceding frame");
  d.decode(zeroKey);
  expect(d.decode(zeroDelta).frame.tick).toBe(11);
  expect(() => d.decode(Buffer.from([0xf8, 0x80, 0, 0]))).toThrow(PackError);
  expect(() => d.decode(Buffer.concat([Buffer.from([24, 0]), zeroKey]))).toThrow("embedded v7 key");
});
