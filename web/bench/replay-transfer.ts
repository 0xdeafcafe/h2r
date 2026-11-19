// cd web && bun bench/replay-transfer.ts <TELEMETRY_BENCH_OUT from bench_match_wire>
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import { decodeChunk } from "../src/api/chunk";
import type { Frame } from "../src/api/types";

const dir = process.argv[2];
if (!dir) throw new Error("pass the bench_match_wire output directory");
const match = JSON.parse(readFileSync(join(dir, "match.json"), "utf8"));
const chunks = Array.from({ length: match.chunks }, (_, i) => {
  const prefix = join(dir, "replay-" + String(i).padStart(3, "0"));
  return { json: readFileSync(prefix + ".json"), binary: readFileSync(prefix + ".bin") };
});
const utf8 = new TextDecoder();
const fromJson = (b: Uint8Array): Frame[] => JSON.parse(utf8.decode(b));

// Validate every endpoint before timing. Oct normals and radians go through
// libm in Rust and JavaScript, so allow a few double-precision ulps.
function checkValue(actual: unknown, expected: unknown): void {
  if (actual === expected) return;
  if (typeof actual === "number" && typeof expected === "number") {
    assert.ok(Math.abs(actual - expected) <= 8 * Number.EPSILON * Math.max(1, Math.abs(expected)), `${actual} versus ${expected}`);
    return;
  }
  assert.ok(actual !== null && typeof actual === "object");
  assert.ok(expected !== null && typeof expected === "object");
  assert.equal(Array.isArray(actual), Array.isArray(expected));
  const keys = Object.keys(expected);
  assert.equal(Object.keys(actual).length, keys.length);
  for (const k of keys) {
    assert.ok(Object.hasOwn(actual, k));
    checkValue((actual as Record<string, unknown>)[k], (expected as Record<string, unknown>)[k]);
  }
}

for (const c of chunks) {
  const actual = await decodeChunk(c.binary);
  const expected = fromJson(c.json);
  checkValue(actual, expected);
}

// Warm both paths, alternate their order, time from the received bytes.
// I/O, output checking and GC between runs are outside the measured spans.
const jsonMs: number[] = [], binaryMs: number[] = [];
for (let run = 0; run < 6; run++) {
  for (const binary of (run % 2 ? [false, true] : [true, false])) {
    Bun.gc(true);
    const start = performance.now();
    let count = 0;
    for (const c of chunks) count += (binary ? await decodeChunk(c.binary) : fromJson(c.json)).length;
    const elapsed = performance.now() - start;
    assert.equal(count, match.frames);
    if (run) (binary ? binaryMs : jsonMs).push(elapsed);
  }
}
const median = (values: number[]) => values.sort((a, b) => a - b)[values.length >> 1]!;
console.log(JSON.stringify({
  ...match,
  replay_json_gzip_bytes: chunks.reduce((n, c) => n + gzipSync(c.json, { level: 6 }).length, 0),
  replay_json_decode_ms: median(jsonMs),
  replay_binary_decode_ms: median(binaryMs),
  bun: Bun.version,
}, null, 2));
