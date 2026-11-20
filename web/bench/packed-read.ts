// bun bench/packed-read.ts <verified H2_V8_DUMP> <bench_match_wire directory>
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { chunkContainer } from "../src/api/chunk";
import { FrameDecoder, records } from "../src/api/pack";
import type { Frame } from "../src/api/types";

const [dump, dir] = process.argv.slice(2);
if (!dump || !dir) throw new Error("pass the production v8 dump and v7 match directory");
const match = JSON.parse(readFileSync(join(dir, "match.json"), "utf8"));
const live = readFileSync(dump);
const packets = records(live);
const v7: Uint8Array[] = [], v8: Uint8Array[] = [];
const joins: number[] = [], keys: number[] = [];
let cursor = 0, at = 0, sinceKey = 0;
const reader = new FrameDecoder();
for (let i = 0; i < match.chunks; i++) {
  const file = join(dir, "replay-" + String(i).padStart(3, "0") + ".bin");
  const raw = await chunkContainer(readFileSync(file));
  v7.push(raw);
  const start = at;
  new FrameDecoder().forEachRecord(raw, (expected: Frame) => {
    const packet = packets[cursor++]!;
    const { frame, keyframe } = reader.decode(packet);
    assert.deepEqual(frame, expected, "frame " + cursor);
    let prefix = 1;
    for (let n = packet.length; n >= 128; n = Math.floor(n / 128)) prefix++;
    const bytes = prefix + packet.length;
    if (keyframe) { keys.push(bytes); sinceKey = 0; }
    sinceKey += bytes;
    joins.push(sinceKey);
    at += bytes;
  });
  v8.push(live.subarray(start, at));
}
assert.equal(cursor, match.frames);
assert.equal(at, live.length);

function measure(containers: Uint8Array[]): number {
  const decoder = new FrameDecoder();
  let n = 0;
  const visit = () => { n++; };
  Bun.gc(true);
  const begin = performance.now();
  for (const bytes of containers) decoder.forEachRecord(bytes, visit);
  const ms = performance.now() - begin;
  assert.equal(n, cursor);
  return ms;
}

const v7ms: number[] = [], v8ms: number[] = [];
for (let i = 0; i < 7; i++) {
  for (const version of (i % 2 ? [7, 8] : [8, 7])) {
    const ms = measure(version === 7 ? v7 : v8);
    if (i) (version === 7 ? v7ms : v8ms).push(ms);
  }
}

function summary(values: number[]) {
  const sorted = values.slice().sort((a, b) => a - b);
  const m = sorted.length >> 1;
  return { min: sorted[0], median: (sorted[m]! + sorted[(sorted.length - 1) >> 1]!) / 2, max: sorted.at(-1) };
}

console.log(JSON.stringify({
  frames: cursor, bun: Bun.version, v7_raw_chunk_bytes: v7.reduce((n, b) => n + b.length, 0),
  v8_live_bytes: at, v8_key_bytes: summary(keys), v8_cached_join_bytes: summary(joins),
  v7_decode_ms: summary(v7ms), v8_decode_ms: summary(v8ms), v7ms, v8ms,
}, null, 2));
