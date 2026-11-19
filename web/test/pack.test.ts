import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { BitReader, FrameDecoder, PackError, decodeKeyframe, fromBase64, records, recordedFrame, unoct } from "../src/api/pack";

const dir = join(import.meta.dir, "fixtures", "pack");
const load = (name: string) => ({
  bin: new Uint8Array(readFileSync(join(dir, name + ".bin"))),
  json: JSON.parse(readFileSync(join(dir, name + ".json"), "utf8")),
});

/** V7 delta header for malformed-record tests. */
function deltaHeader(key: { sid: number; seq: number }) {
  const bits: number[] = [];
  const u = (n: number, width: number) => {
    for (let i = 0; i < width; i++) bits.push((n >> i) & 1);
  };
  const eg = (n: number, k: number) => {
    const x = n + 2 ** k;
    const width = Math.floor(Math.log2(x));
    u(0, width - k);
    u(1, 1);
    u(x, width);
  };
  u(7, 4); u(0, 1); eg(key.sid, 0); eg(key.seq + 1, 8);
  eg(1, 2); u(0, 3); eg(0, 1); // dt, no BSP change, cut or cinematic, zero strings
  const bytes = () => {
    const record = new Uint8Array(Math.ceil(bits.length / 8));
    bits.forEach((bit, i) => { record[i >> 3]! |= bit << (i & 7); });
    return record;
  };
  return { eg, u, bytes };
}

describe("packed frames", () => {
  for (const name of ["multiplayer", "campaign", "odd", "cinematic", "v1-multiplayer", "v2-multiplayer", "v3-multiplayer", "v4-multiplayer", "v4-cinematic", "v6-multiplayer", "v6-campaign", "v6-odd", "v6-cinematic", "v7-multiplayer", "v7-campaign", "v7-odd", "v7-cinematic", "v7-motion"]) {
    test(`decodes the Rust encoder's ${name} stream exactly`, () => {
      const { bin, json } = load(name);
      const d = new FrameDecoder();
      const frames = records(bin).map((r) => d.decode(r).frame);
      expect(frames).toEqual(json);
    });
  }

  test("a turret keeps its parent and aim; version 1 vehicles have neither", () => {
    const turret = load("multiplayer").json[0].vehicles.find((v: { kind: string }) => v.kind === "chaingun");
    expect(turret.parent).toBe("90040004");
    expect(turret.aim).toHaveLength(3);
    const d = new FrameDecoder();
    const v1 = records(load("v1-multiplayer").bin).map((r) => d.decode(r).frame);
    expect(v1[0]!.vehicles!.length).toBeGreaterThan(0);
    expect(v1.flatMap((f) => f.vehicles ?? []).every((v) => v.parent === undefined && v.aim === undefined)).toBe(true);
  });

  test("version 3 fields; version 2 frames have none", () => {
    const odd = load("odd").json[0];
    const d = new FrameDecoder();
    const p = d.decode(records(load("odd").bin)[0]!).frame.players.find((x) => x.index === 15)!;
    expect(odd.cut).toBe(123456);
    expect(p.fire).toBe(2 ** 32 - 1);
    expect(p.vel).toEqual([-999, 0.015625, 12.5]);
    expect(p.seat).toBe(63);
    expect(p.actions_ago).toEqual([900, 31, 30, 7, 2, 1, 0, 0]);
    expect(p.anim).toEqual({ name: "s_kill:front:gut-2", frame: 32767 });
    const d3 = new FrameDecoder();
    const v3 = records(load("v3-multiplayer").bin).map((r) => d3.decode(r).frame);
    expect(v3.every((f) => f.players.every((x) => x.anim === undefined && x.vel !== undefined))).toBe(true);
    const d2 = new FrameDecoder();
    const v2 = records(load("v2-multiplayer").bin).map((r) => d2.decode(r).frame);
    expect(v2.every((f) => f.cut === undefined && f.players.every((x) => x.vel === undefined && x.fire === undefined))).toBe(true);
    // A version 2 delta never follows on from a version 3 frame.
    const mixed = new FrameDecoder();
    mixed.decode(records(load("multiplayer").bin)[0]!);
    expect(() => mixed.decode(records(load("v2-multiplayer").bin)[1]!)).toThrow(PackError);
  });

  test("before version 6, cinematic frames were version 5 keyframes in and out; the rest version 4", () => {
    const recs = records(load("v4-cinematic").bin);
    // Version in the low four bits, the keyframe bit next.
    expect(recs.map((r) => r[0]! & 0xf)).toEqual([4, 5, 5, 4]);
    expect(recs.map((r) => (r[0]! >> 4) & 1)).toEqual([1, 1, 0, 1]);
    const d = new FrameDecoder();
    const frames = recs.map((r) => d.decode(r).frame);
    expect(frames.map((f) => f.cinematic === true)).toEqual([false, true, true, false]);
    expect(frames.every((f) => f.players.length === 0 && f.actors === undefined)).toBe(true);
  });

  test("version 6: every frame carries the cinematic bit, so a cutscene is deltas", () => {
    const recs = records(load("v6-cinematic").bin);
    expect(recs.map((r) => r[0]! & 0xf)).toEqual([6, 6, 6, 6]);
    expect(recs.map((r) => (r[0]! >> 4) & 1)).toEqual([1, 0, 0, 0]);
    const d = new FrameDecoder();
    expect(recs.map((r) => d.decode(r).frame.cinematic === true)).toEqual([false, true, true, false]);
  });

  test("version 6 projectiles, items and vehicle velocity; version 4 frames have none", () => {
    const d = new FrameDecoder();
    const odd = d.decode(records(load("v6-odd").bin)[0]!).frame;
    expect(odd.projectiles).toHaveLength(2);
    expect(odd.projectiles![0]!.parent).toBe("p15");
    expect(odd.projectiles![0]!.vel).toBeUndefined();
    expect(odd.projectiles![1]!.vel).toEqual([-99, 0.5, -0.015625]);
    expect(odd.items![0]!.equipment).toBe(true);
    const mp = new FrameDecoder();
    const frames = records(load("v6-multiplayer").bin).map((r) => mp.decode(r).frame);
    expect(frames.every((f) => (f.items ?? []).length >= 4)).toBe(true);
    expect(frames[0]!.vehicles!.filter((v) => v.kind !== "chaingun").every((v) => v.vel !== undefined)).toBe(true);
    const v4 = new FrameDecoder();
    const old = records(load("v4-multiplayer").bin).map((r) => v4.decode(r).frame);
    expect(old.every((f) => f.projectiles === undefined && f.items === undefined && (f.vehicles ?? []).every((v) => v.vel === undefined))).toBe(true);
    // A version 4 delta never follows on from a version 6 frame.
    const mixed = new FrameDecoder();
    mixed.decode(records(load("v6-multiplayer").bin)[0]!);
    expect(() => mixed.decode(records(load("v4-multiplayer").bin)[1]!)).toThrow(PackError);
  });

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

  test("v7 shares unchanged vectors and seeks at the cut after an irregular gap", () => {
    const { bin, json } = load("v7-motion");
    const d = new FrameDecoder();
    const sequence: ReturnType<typeof d.decode>[] = [];
    d.forEachRecord(bin, (frame, sid, seq, keyframe) => sequence.push({ frame, sid, seq, keyframe }));
    expect(sequence.map(f => f.frame.tick)).toEqual([0, 1, 3, 10, 42, 44, 45]);
    expect(sequence.map(f => f.frame)).toEqual(json);
    expect(sequence[1]!.frame.players[0]!.vel).toBe(sequence[0]!.frame.players[0]!.vel);
    const seek = new FrameDecoder();
    const rs = records(bin);
    expect(seek.decode(rs[5]!).frame).toEqual(json[5]);
    expect(seek.decode(rs[6]!).frame).toEqual(json[6]);
  });

  test("a unit edit mask must end by its fifth group", () => {
    const d = new FrameDecoder();
    const key = d.decode(records(load("v7-motion").bin)[0]!);
    const { eg, u, bytes } = deltaHeader(key);
    eg(1, 2); u(1, 1); // one matched player at the previous cursor
    for (let i = 0; i < 5; i++) u(16, 5);
    expect(() => d.decode(bytes())).toThrow("unit mask too long");
    // A rejected delta leaves the stream usable at the same sequence.
    expect(d.decode(records(load("v7-motion").bin)[1]!).frame.tick).toBe(1);
  });

  test("a hit-position reference is bounded by this frame's player count", () => {
    const d = new FrameDecoder();
    const key = d.decode(records(load("v7-motion").bin)[0]!);
    const { eg, u, bytes } = deltaHeader(key);
    eg(1, 2); u(0, 2); eg(0, 2); // keep only the first player
    eg(0, 2); eg(0, 2); // vehicles and actors
    eg(1, 2); u(0, 3); // one hit without source, target or weapon
    u(1, 1); u(15, 4);
    expect(() => d.decode(bytes())).toThrow("hit refers past the player list");
  });

  test("container reads cannot borrow bytes from the following record", () => {
    const good = records(load("multiplayer").bin)[0]!;
    const bad = new Uint8Array(3 + good.length);
    bad.set([1, good[0]!, 0]);
    bad.set(good, 3);
    expect(() => new FrameDecoder().forEachRecord(bad, () => { throw new Error("unexpected record"); })).toThrow("ends early");
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

  test("a delta shares the frame objects of entities it did not change", () => {
    const { bin, json } = load("campaign");
    const d = new FrameDecoder();
    const frames = records(bin).map((r) => d.decode(r).frame);
    let shared = 0, fresh = 0;
    for (let i = 1; i < frames.length; i++) {
      if (records(bin)[i]![0]! & 0x10) continue; // a keyframe makes all anew
      const before = new Set<unknown>(frames[i - 1]!.actors ?? []);
      for (const a of frames[i]!.actors ?? []) before.has(a) ? shared++ : fresh++;
    }
    expect(shared).toBeGreaterThan(0);
    expect(fresh).toBeGreaterThan(0);
    // Sharing changes nothing.
    expect(frames).toEqual(json);
  });

  test("bit reader codes, every width at every bit offset", () => {
    const bytes = new Uint8Array(16).map((_, i) => (i * 0x9d + 0x35) & 0xff);
    const at = (pos: number, n: number) => {
      let v = 0;
      for (let k = 0; k < n; k++) v += ((bytes[(pos + k) >> 3]! >> ((pos + k) & 7)) & 1) * 2 ** k;
      return v;
    };
    for (let n = 0; n <= 32; n++) {
      for (let pos = 0; pos + n <= bytes.length * 8; pos += 3) {
        const r = new BitReader(bytes);
        if (pos) r.bits(pos % 24), pos % 24 === pos || r.bits(pos - (pos % 24));
        expect(r.bits(n)).toBe(at(pos, n));
      }
    }
    const r = new BitReader(bytes);
    r.bits(24);
    expect(() => r.bits(24 * 4 + 9)).toThrow(PackError);
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
