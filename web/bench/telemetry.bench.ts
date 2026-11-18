// Web-side live telemetry benchmark. Run: `bun run bench:telemetry`
// (cd web). Not a test: the name does not match *.test.ts, and bunfig.toml
// roots `bun test` at ./test.
//
// Input: pack.ts only has a decoder, so this file carries a small encoder
// that mirrors it (same schemas, bit order, delta runs/edits). A synthetic
// 600-frame stream (a keyframe every 60 frames, deltas between, 15 Hz) of
// 16 players, 64 vehicles (16 of them turrets with parent and aim), 100
// actors, ~200 scenario objects (20 moving), 40 machines (10 moving) and
// 0-3 hits a frame, all moving smoothly, is encoded, then decoded back with
// the real FrameDecoder (and sanity-checked against the model it came from).
//
// Measured, median microseconds per frame:
//   decode  FrameDecoder.decode of one packed record (deltas and keyframes)
//   reduce  FrameHub.push of one decoded frame (state/frames.ts; the live
//           "stream reducer" for frames: ring buffer, machines, bounds, clock)
//   chunk   decodeChunk of one ten second recorded chunk (150 frames, one
//           keyframe then deltas, raw deflate; api/chunk.ts), per chunk:
//           the replay target is at most 2 ms in V8 on one core
//           (docs/live/replay-fidelity.md, "Compression")
//   derive  the per-rendered-frame pure sampling the map/3D viewers do each
//           repaint (lib/game.ts liveFrameSource: sample, sampleActors,
//           sampleVehicles, sampleHits, sampleBsp, sampleMachines,
//           sampleObjects). No WebGL; players3d/turrets3d are three.js-bound
//           and left out.
// TELEMETRY_BENCH_OUT=<dir> also writes <dir>/web.json.

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { deflateRawSync } from "node:zlib";
import { decodeChunk } from "../src/api/chunk";
import { FrameDecoder, OBJECT_KINDS } from "../src/api/pack";
import type { Frame } from "../src/api/types";
import { FrameBuffer, FrameHub } from "../src/state/frames";

const FRAMES = 600;
const KEY_EVERY = 60;
const WARMUP_PASSES = 4;
const PASSES = 12;
const TICKS_PER_FRAME = 2;
const MS_PER_FRAME = 1000 / 15;
const RENDERS_PER_FRAME = 4; // ~60 Hz repaints between 15 Hz frames

// ---------------------------------------------------------------- encoder

class BitWriter {
  private buf = new Uint8Array(1 << 16);
  private pos = 0;
  reset() { this.pos = 0; this.buf.fill(0); }
  bit(b: boolean | number) { this.bits(b ? 1 : 0, 1); }
  bits(v: number, n: number) {
    let left = n;
    while (left > 0) {
      const off = this.pos & 7;
      const take = Math.min(8 - off, left);
      const chunk = v % 2 ** take;
      this.buf[this.pos >> 3]! |= chunk << off;
      v = Math.floor(v / 2 ** take);
      left -= take;
      this.pos += take;
    }
  }
  eg(v: number, k: number) {
    const x = v + 2 ** k;
    const z = Math.floor(Math.log2(x)) - k;
    for (let i = 0; i < z; i++) this.bit(0);
    this.bit(1);
    this.bits(x - 2 ** (z + k), z + k);
  }
  seg(v: number, k: number) { this.eg(v >= 0 ? 2 * v : -2 * v - 1, k); }
  finish(): Uint8Array { return this.buf.slice(0, (this.pos + 7) >> 3); }
}

const enum K { Bool, Pos, Angle, Dir, Str, Id, Uint, Bits, Actions }
interface Field { k: K; arg?: number; opt?: boolean; key?: boolean }
const f = (k: K, arg?: number): Field => ({ k, arg });
const o = (k: K, arg?: number): Field => ({ k, arg, opt: true });
const key = (k: K, arg?: number): Field => ({ k, arg, key: true });

const PLAYER = [key(K.Uint, 2), f(K.Bool), f(K.Pos), f(K.Angle), o(K.Id), o(K.Str), o(K.Dir), o(K.Str), o(K.Uint, 1), o(K.Bits, 3), o(K.Uint, 6), o(K.Actions), o(K.Bits, 1)];
const VEHICLE = [key(K.Id), f(K.Str), f(K.Pos), f(K.Dir), f(K.Dir), f(K.Bool), o(K.Id), o(K.Dir)];
const ACTOR = [key(K.Id), f(K.Str), f(K.Pos), f(K.Angle), o(K.Dir), o(K.Str), o(K.Uint, 1), o(K.Str), o(K.Id), o(K.Bits, 3), o(K.Uint, 6), o(K.Actions), o(K.Bits, 1)];
const HIT = [o(K.Id), o(K.Id), o(K.Str), f(K.Pos), o(K.Uint, 6), o(K.Str)];
const MACHINE = [o(K.Str), key(K.Id), f(K.Str), f(K.Pos), f(K.Dir), f(K.Dir), o(K.Bits, 10)];
const OBJECT = [key(K.Bits, 4), key(K.Uint, 6), f(K.Bool), o(K.Pos), o(K.Dir), o(K.Dir)];

type V = unknown;
type Entity = V[];
type Id = { datum: number } | { player: number } | { text: string };
type Pos = [number, number, number];

interface Model { tick: number; players: Entity[]; vehicles: Entity[]; actors: Entity[]; hits: Entity[]; machines: Entity[]; objects: Entity[] }

class Encoder {
  private w = new BitWriter();
  private strings = new Map<string, number>();
  private fresh: string[] = [];
  seq = 0;

  private intern(s: string) {
    if (!this.strings.has(s)) { this.strings.set(s, this.strings.size); this.fresh.push(s); }
  }
  private scan(fields: Field[], e: Entity) {
    fields.forEach((fl, i) => {
      const v = e[i];
      if (v === null || v === undefined) return;
      if (fl.k === K.Str) this.intern(v as string);
      else if (fl.k === K.Id && "text" in (v as Id)) this.intern((v as { text: string }).text);
    });
  }
  private strIdx(s: string) { return this.strings.get(s)!; }

  private value(fl: Field, v: V, prev: V) {
    const w = this.w;
    switch (fl.k) {
      case K.Bool: w.bit(v as boolean); break;
      case K.Pos: {
        const p = v as Pos;
        if (Array.isArray(prev) && prev.length === 3) for (let i = 0; i < 3; i++) w.seg(p[i]! - (prev as Pos)[i]!, 7);
        else for (let i = 0; i < 3; i++) w.seg(p[i]!, 16);
        break;
      }
      case K.Angle: w.bits(v as number, 12); break;
      case K.Dir: w.bits((v as number[])[0]!, 12); w.bits((v as number[])[1]!, 12); break;
      case K.Str: w.eg(this.strIdx(v as string), 3); break;
      case K.Id: {
        const id = v as Id;
        if ("datum" in id) { w.bits(0, 2); w.bits(id.datum, 32); }
        else if ("player" in id) { w.bits(1, 2); w.eg(id.player, 2); }
        else { w.bits(2, 2); w.eg(this.strIdx(id.text), 3); }
        break;
      }
      case K.Uint: w.eg(v as number, fl.arg!); break;
      case K.Bits: w.bits(v as number, fl.arg!); break;
      case K.Actions: { const a = v as number[]; w.eg(a.length, 0); for (const x of a) w.bits(x, 3); break; }
    }
  }
  private field(fl: Field, v: V, prev: V) {
    if (fl.opt) { this.w.bit(v !== null); if (v === null) return; }
    this.value(fl, v, prev);
  }
  private entity(fields: Field[], e: Entity) { fields.forEach((fl, i) => this.field(fl, e[i], undefined)); }

  private full(fields: Field[], list: Entity[]) {
    this.w.eg(list.length, 2);
    for (const e of list) this.entity(fields, e);
  }

  private delta(fields: Field[], prev: Entity[], list: Entity[]) {
    const w = this.w;
    w.eg(list.length, 2);
    let run = 0;
    const flush = () => { if (run) { w.bit(0); w.eg(run - 1, 2); run = 0; } };
    list.forEach((e, i) => {
      const p = prev[i]!;
      const changed = fields.map((fl, j) => !fl.key && JSON.stringify(e[j]) !== JSON.stringify(p[j]));
      if (!changed.some(Boolean)) { run++; return; }
      flush();
      w.bit(1); w.bit(1); w.seg(0, 0);
      for (let j = 0; j < fields.length; j++) if (!fields[j]!.key) w.bit(changed[j]!);
      fields.forEach((fl, j) => { if (changed[j]) this.field(fl, e[j], p[j]); });
    });
    flush();
  }

  encode(m: Model, prev: Model | null, keyframe: boolean): Uint8Array {
    const w = this.w;
    w.reset();
    if (keyframe) { this.strings.clear(); this.seq = 0; }
    this.fresh = [];
    for (const [fl, list] of [[PLAYER, m.players], [VEHICLE, m.vehicles], [ACTOR, m.actors], [HIT, m.hits], [MACHINE, m.machines], [OBJECT, m.objects]] as const) {
      for (const e of list) this.scan(fl as Field[], e);
    }
    w.bits(2, 4); w.bit(keyframe); w.eg(7, 0); w.eg(this.seq, 8);
    const lit = (s: string) => { const b = new TextEncoder().encode(s); w.eg(b.length, 3); for (const c of b) w.bits(c, 8); };
    if (keyframe) { lit("bench-game"); w.eg(m.tick, 20); w.bit(1); w.eg(3, 1); }
    else { w.eg(m.tick - prev!.tick, 2); w.bit(0); }
    w.eg(this.fresh.length, 1);
    for (const s of this.fresh) lit(s);
    const pairs: [Field[], Entity[], Entity[] | undefined][] = [
      [PLAYER, m.players, prev?.players], [VEHICLE, m.vehicles, prev?.vehicles], [ACTOR, m.actors, prev?.actors],
    ];
    for (const [fl, list, p] of pairs) keyframe ? this.full(fl, list) : this.delta(fl, p!, list);
    this.full(HIT, m.hits);
    keyframe ? this.full(MACHINE, m.machines) : this.delta(MACHINE, prev!.machines, m.machines);
    keyframe ? this.full(OBJECT, m.objects) : this.delta(OBJECT, prev!.objects, m.objects);
    this.seq++;
    return w.finish();
  }
}

// ------------------------------------------------------------- simulation

let seed = 0x2545f491;
const rnd = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 2 ** 32; };
const q = (x: number): number => Math.round(x * 512);
const dirOf = (a: number, b: number): [number, number] => [Math.round(2048 + 1800 * Math.cos(a) * Math.cos(b)), Math.round(2048 + 1800 * Math.sin(a))];
const WEAPONS = ["battle_rifle", "smg", "plasma_pistol", "shotgun", "sniper_rifle", "rocket_launcher"];
const SPECIES = ["spartan", "elite"];
const ACTOR_SPECIES = ["grunt", "marine", "elite", "jackal", "flood_combat"];
const TEAMS = ["covenant", "human", "flood"];
const VEHICLE_KINDS = ["warthog", "ghost", "scorpion", "banshee", "wraith"];
const datum = (hi: number, n: number): Id => ({ datum: hi * 65536 + n });

function ring(i: number, t: number, r: number, w: number): Pos {
  const a = i * 0.9 + t * w;
  return [q(Math.cos(a) * r + (i % 7) * 4), q(Math.sin(a) * r + (i % 5) * 4), q(1 + 0.2 * Math.sin(t * 2 + i))];
}

function simulate(frames: number): Model[] {
  const out: Model[] = [];
  const objectsTotal = 200, machinesTotal = 40;
  for (let s = 0; s < frames; s++) {
    const t = s * MS_PER_FRAME / 1000;
    const players: Entity[] = [];
    for (let i = 0; i < 16; i++) {
      const shot = rnd() < 0.15;
      const acts: number[] = rnd() < 0.04 ? [(rnd() * 6) | 0] : [];
      players.push([
        i, i !== 3 || s % 200 > 20, ring(i, t, 40 + i * 2, 0.35), Math.floor(((i * 0.9 + t * 0.35) % (2 * Math.PI)) / (2 * Math.PI) * 4096) % 4096,
        i === 5 ? datum(0x9004, 7) : null, SPECIES[i & 1]!, dirOf(t + i, Math.sin(t) * 0.3), WEAPONS[(i + (s / 80 | 0)) % WEAPONS.length]!,
        shot ? 1 + ((rnd() * 3) | 0) : null, i === 5 ? 3 : (i + (s >> 3)) % 3, Math.round(16 * (3 + Math.sin(t + i))), acts.length ? acts : null, rnd() < 0.01 ? 0 : null,
      ]);
    }
    const vehicles: Entity[] = [];
    for (let i = 0; i < 64; i++) {
      const turret = i >= 48;
      const pi = i - 48; // turret i mounts plain vehicle pi
      vehicles.push([
        datum(0x9004, i), turret ? "chaingun" : VEHICLE_KINDS[i % VEHICLE_KINDS.length]!,
        turret ? ring(pi, t, 60 + pi, 0.12) : ring(i + 20, t, 60 + i, 0.12),
        dirOf(t * 0.5 + i, 0), dirOf(0, 0), !turret && i % 3 === 0,
        turret ? datum(0x9004, pi) : null, turret ? dirOf(t + i, 0.2) : null,
      ]);
    }
    const actors: Entity[] = [];
    for (let i = 0; i < 100; i++) {
      const moving = i % 4 !== 0;
      actors.push([
        datum(0x9005, i), ACTOR_SPECIES[i % ACTOR_SPECIES.length]!,
        moving ? ring(i + 40, t, 25 + (i % 30), 0.2) : [q(10 + i), q(-20), q(0.5)],
        moving ? Math.floor((((i + t) * 0.5) % (2 * Math.PI)) / (2 * Math.PI) * 4096) : 0,
        moving ? dirOf(t + i, 0) : null, i % 2 ? "plasma_rifle" : null, rnd() < 0.05 ? 1 : null, TEAMS[i % 3]!,
        null, i % 4, moving ? 48 : 0, null, null,
      ]);
    }
    const hits: Entity[] = [];
    for (let h = (rnd() * 4) | 0; h > 0; h--) hits.push([{ player: (rnd() * 16) | 0 }, { player: (rnd() * 16) | 0 }, WEAPONS[(rnd() * 6) | 0]!, ring(h, t, 40, 0.3), (rnd() * 256) | 0, "bullet"]);
    const machines: Entity[] = [];
    for (let i = 0; i < machinesTotal; i++) {
      const moving = i < 10;
      machines.push([`door_${i}`, datum(0x9006, i), "door", [q(i * 3), q(i), q(moving ? 1 + Math.abs(Math.sin(t + i)) : 1)], dirOf(0, 0), dirOf(0, 1), moving ? Math.round(1023 * Math.abs(Math.sin(t + i))) : 0]);
    }
    const objects: Entity[] = [];
    for (let i = 0; i < objectsTotal; i++) {
      const moving = i < 20;
      objects.push([i % OBJECT_KINDS.length, i, false, moving ? [q(i + Math.sin(t) * 3), q(i), q(0.5)] : [q(i), q(i), q(0.5)], dirOf(0, 0), dirOf(0, 1)]);
    }
    out.push({ tick: 1000 + s * TICKS_PER_FRAME, players, vehicles, actors, hits, machines, objects });
  }
  return out;
}

// ------------------------------------------------------------------ stats

const median = (xs: number[]) => { const s = [...xs].sort((a, b) => a - b); return s[s.length >> 1] ?? 0; };
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))] ?? 0; };
const mean = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / (xs.length || 1);
const us = (ms: number) => ms * 1000;

interface Row { medianUs: number; p95Us: number; meanUs: number; framesPerSec: number; samples: number }
function row(msSamples: number[]): Row {
  const m = median(msSamples);
  return { medianUs: us(m), p95Us: us(pct(msSamples, 0.95)), meanUs: us(mean(msSamples)), framesPerSec: m > 0 ? 1000 / m : Infinity, samples: msSamples.length };
}
const heap = () => { Bun.gc(true); return process.memoryUsage().heapUsed; };

// ------------------------------------------------------------------- main

const t0 = performance.now();
const model = simulate(FRAMES);
const enc = new Encoder();
const records: Uint8Array[] = model.map((m, i) => enc.encode(m, i % KEY_EVERY === 0 ? null : model[i - 1]!, i % KEY_EVERY === 0));
const keyIdx = records.map((_, i) => i).filter((i) => i % KEY_EVERY === 0);
const bytes = records.reduce((a, r) => a + r.length, 0);
const keyBytes = keyIdx.reduce((a, i) => a + records[i]!.length, 0);

// Sanity check the encoder against the real decoder.
const frames: Frame[] = [];
{
  const d = new FrameDecoder();
  records.forEach((r, i) => {
    const fr = d.decode(r).frame, m = model[i]!;
    if (fr.tick !== m.tick || fr.players.length !== 16 || fr.vehicles?.length !== 64 || fr.actors?.length !== 100 || fr.machines?.length !== 40 || fr.objects?.length !== 200) throw new Error(`bench encoder disagrees with decoder at frame ${i}`);
    const p = fr.players[7]!.pos, e = m.players[7]![2] as Pos;
    if (Math.abs(p[0] - e[0] / 512) > 1e-9 || Math.abs(p[2] - e[2] / 512) > 1e-9) throw new Error(`position mismatch at frame ${i}`);
    if (fr.vehicles![50]!.parent === undefined) throw new Error("turret lost its parent");
    frames.push(fr);
  });
}
const setupMs = performance.now() - t0;

// 1. decode
const decDelta: number[] = [], decKey: number[] = [], decAll: number[] = [];
const heapBeforeDecode = heap();
for (let pass = -WARMUP_PASSES; pass < PASSES; pass++) {
  const d = new FrameDecoder();
  for (let i = 0; i < records.length; i++) {
    const a = performance.now();
    d.decode(records[i]!);
    const dt = performance.now() - a;
    if (pass >= 0) { decAll.push(dt); (i % KEY_EVERY === 0 ? decKey : decDelta).push(dt); }
  }
}
const heapAfterDecode = heap();

// 2. reduce: FrameHub.push of decoded frames, a fresh hub each pass.
const reduce: number[] = [];
let retained = 0;
for (let pass = -WARMUP_PASSES; pass < PASSES; pass++) {
  const base = pass >= 0 && pass === PASSES - 1 ? heap() : 0;
  const hub = new FrameHub();
  for (let i = 0; i < frames.length; i++) {
    const a = performance.now();
    hub.push(frames[i]!, i * MS_PER_FRAME);
    const dt = performance.now() - a;
    if (pass >= 0) reduce.push(dt);
  }
  if (base) retained = heap() - base;
  hub.clear();
}

// 3. derive: the samplers a repaint runs, at ~60 Hz between 15 Hz frames.
const names = ["sample", "sampleActors", "sampleVehicles", "sampleHits", "sampleBsp", "sampleMachines", "sampleObjects"] as const;
const parts: Record<(typeof names)[number], number[]> = { sample: [], sampleActors: [], sampleVehicles: [], sampleHits: [], sampleBsp: [], sampleMachines: [], sampleObjects: [] };
const derive: number[] = [];
for (let pass = -WARMUP_PASSES; pass < PASSES; pass++) {
  const buf = new FrameBuffer();
  for (let i = 0; i < frames.length; i++) {
    const at = i * MS_PER_FRAME;
    buf.push(frames[i]!, at);
    if (i < 10) continue;
    for (let r = 0; r < RENDERS_PER_FRAME; r++) {
      const now = at + (r * MS_PER_FRAME) / RENDERS_PER_FRAME;
      const a = performance.now();
      let prev = a;
      for (const n of names) {
        buf[n](now);
        const b = performance.now();
        if (pass >= 0) parts[n].push(b - prev);
        prev = b;
      }
      if (pass >= 0) derive.push(prev - a);
    }
  }
}

// 4. chunk: the first 150 frames as one recorded chunk.
const CHUNK_FRAMES = 150;
const chunkEnc = new Encoder();
const chunkRecs = model.slice(0, CHUNK_FRAMES).map((m, i) => chunkEnc.encode(m, i === 0 ? null : model[i - 1]!, i === 0));
const leb = (n: number): number[] => { const out: number[] = []; for (;;) { const b = n & 0x7f; n = Math.floor(n / 128); if (n === 0) return [...out, b]; out.push(b | 0x80); } };
const rawChunk = Uint8Array.from(chunkRecs.flatMap((r) => [...leb(r.length), ...r]));
const deflated = new Uint8Array(deflateRawSync(rawChunk, { level: 6 }));
const chunkBytes = Uint8Array.from([0x48, 0x32, 0x43, 0x4b, 1, 1, ...leb(rawChunk.length), ...leb(CHUNK_FRAMES), ...leb(0), ...leb((CHUNK_FRAMES - 1) * TICKS_PER_FRAME), ...deflated]);
const chunkMs: number[] = [];
for (let pass = -WARMUP_PASSES; pass < PASSES; pass++) {
  const a = performance.now();
  const got = await decodeChunk(chunkBytes);
  if (pass >= 0) chunkMs.push(performance.now() - a);
  if (got.length !== CHUNK_FRAMES) throw new Error("chunk decoded short");
}

const result = {
  bun: Bun.version,
  input: {
    frames: FRAMES, keyframeEvery: KEY_EVERY, ticksPerFrame: TICKS_PER_FRAME, hz: 15,
    players: 16, vehicles: 64, turrets: 16, actors: 100, machines: 40, objects: 200,
    totalBytes: bytes, avgBytesPerFrame: Math.round(bytes / FRAMES), keyframeBytes: Math.round(keyBytes / keyIdx.length),
    deltaBytes: Math.round((bytes - keyBytes) / (FRAMES - keyIdx.length)),
    source: "synthetic stream encoded by a bench-local encoder mirroring pack.ts, decoded by the real FrameDecoder",
  },
  decode: { all: row(decAll), delta: row(decDelta), keyframe: row(decKey) },
  reduce: row(reduce),
  chunk: { ...row(chunkMs), frames: CHUNK_FRAMES, rawBytes: rawChunk.length, storedBytes: chunkBytes.length },
  derive: { perRepaint: row(derive), parts: Object.fromEntries(names.map((n) => [n, row(parts[n])])) },
  heap: { decodeLeakBytes: heapAfterDecode - heapBeforeDecode, hubRetainedBytes: retained },
  setupMs: Math.round(setupMs),
};

const fmt = (r: Row) => `${r.medianUs.toFixed(1).padStart(9)} us  p95 ${r.p95Us.toFixed(1).padStart(9)}  mean ${r.meanUs.toFixed(1).padStart(9)}  ${Math.round(r.framesPerSec).toString().padStart(8)}/s`;
console.log(`telemetry web bench (bun ${Bun.version}); ${FRAMES} frames, ${result.input.avgBytesPerFrame} B/frame (key ${result.input.keyframeBytes}, delta ${result.input.deltaBytes}); ${PASSES} passes after ${WARMUP_PASSES} warmup`);
console.log(`decode  all      ${fmt(result.decode.all)}`);
console.log(`decode  delta    ${fmt(result.decode.delta)}`);
console.log(`decode  keyframe ${fmt(result.decode.keyframe)}`);
console.log(`reduce  push     ${fmt(result.reduce)}`);
console.log(`chunk   decode   ${fmt(result.chunk)}  (${CHUNK_FRAMES} frames, ${result.chunk.rawBytes} B raw, ${result.chunk.storedBytes} B stored; per chunk)`);
console.log(`derive  repaint  ${fmt(result.derive.perRepaint)}`);
for (const n of names) console.log(`   ${n.padEnd(15)}${fmt(result.derive.parts[n]!)}`);
console.log(`heap: decode retained ${result.heap.decodeLeakBytes} B after gc; hub of 48 frames retains ${(result.heap.hubRetainedBytes / 1024).toFixed(0)} KiB`);

const outDir = process.env.TELEMETRY_BENCH_OUT;
if (outDir) {
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, "web.json"), JSON.stringify(result, null, 2) + "\n");
}
