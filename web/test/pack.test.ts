import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { BitReader, FrameDecoder, PackError, decodeKeyframe, fromBase64, records, recordedFrame, unoct } from "../src/api/pack";

const dir = join(import.meta.dir, "fixtures", "pack");
const load = (name: string) => ({
  bin: new Uint8Array(readFileSync(join(dir, name + ".bin"))),
  json: JSON.parse(readFileSync(join(dir, name + ".json"), "utf8")),
});

describe("packed frames", () => {
  for (const name of ["multiplayer", "campaign", "odd"]) {
    test(`decodes the Rust encoder's ${name} stream exactly`, () => {
      const { bin, json } = load(name);
      const d = new FrameDecoder();
      const frames = records(bin).map((r) => d.decode(r).frame);
      expect(frames).toEqual(json);
    });
  }

  test("a dropped delta waits for the next keyframe", () => {
    // multiplayer.bin: keyframe every 4 frames, six frames.
    const recs = records(load("multiplayer").bin);
    const { json } = load("multiplayer");
    const d = new FrameDecoder();
    d.decode(recs[0]!);
    let gap: unknown = null;
    try { d.decode(recs[2]!); } catch (e) { gap = e; }
    expect(gap).toBeInstanceOf(PackError);
    expect((gap as PackError).gap).toBe(true);
    expect(() => d.decode(recs[3]!)).toThrow();
    expect(d.decode(recs[4]!).frame).toEqual(json[4]);
    expect(d.decode(recs[5]!).frame).toEqual(json[5]);
  });

  test("keyframes decode alone; deltas do not", () => {
    const recs = records(load("campaign").bin);
    expect(decodeKeyframe(recs[0]!).tick).toBe(load("campaign").json[0].tick);
    expect(() => decodeKeyframe(recs[1]!)).toThrow();
  });

  test("recorded GameFrame data: packed or old JSON", () => {
    const recs = records(load("odd").bin);
    let b64 = "";
    for (const b of recs[0]!) b64 += String.fromCharCode(b);
    const packed = btoa(b64).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    expect(fromBase64(packed)).toEqual(recs[0]!);
    expect(recordedFrame({ tick: 77, packed })).toEqual(load("odd").json[0]);
    const old = { game_id: "g", tick: 5, players: [] };
    expect(recordedFrame(old)).toEqual(old);
    expect(recordedFrame({ tick: 1, packed: "!!!" })).toBeNull();
    expect(recordedFrame(null)).toBeNull();
  });

  test("garbage throws a PackError, never anything else", () => {
    const good = records(load("campaign").bin)[0]!;
    let seed = 12345;
    const rand = () => (seed = (seed * 1103515245 + 12345) % 2 ** 31);
    for (let i = 0; i < 2000; i++) {
      const bytes = good.slice(0, rand() % good.length);
      const at = rand() % Math.max(1, bytes.length);
      if (i % 2 && at < bytes.length) bytes[at] = bytes[at]! ^ (1 << (rand() % 8));
      try { new FrameDecoder().decode(bytes); } catch (e) { expect(e).toBeInstanceOf(PackError); }
    }
  });

  test("bit reader codes", () => {
    // 0b1 (eg k0 = 0), then 0b010 = eg k0 of 1 read LSB first: bits 1,0,1,0 ...
    const r = new BitReader(new Uint8Array([0b00001011, 0xff]));
    expect(r.bit()).toBe(true);
    expect(r.bits(3)).toBe(0b101);
    expect(r.bits(12)).toBe(0xff0);
    expect(() => r.bit()).toThrow(PackError);
    expect(unoct(2047, 2047)).toEqual([0, 0, 1]);
  });
});
