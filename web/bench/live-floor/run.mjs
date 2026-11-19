// bun web/bench/live-floor/run.mjs <bench_match_wire dump directory>
// Timer starts at the decoded v7 frame. It includes projection, prediction,
// model updates and packet framing, excluding JSON parsing and verification.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { Connection } from "./stream.mjs";
const dir = process.argv[2];
if (!dir) throw Error("pass bench_match_wire's TELEMETRY_BENCH_OUT directory");
const stat = JSON.parse(readFileSync(join(dir, "match.json")));
assert.equal(stat.wire_bytes, 1394368);
const files = readdirSync(dir)
  .filter((f) => /^replay-\d+\.json$/.test(f))
  .sort();
const frames = files.flatMap((f) => JSON.parse(readFileSync(join(dir, f))));
assert.equal(frames.length, 9000);
// Removing only the cross-player predictor keeps this a same-data ablation.
for (const [interval, shareAim] of [
  [150, true],
  [450, true],
  [Infinity, true],
  [150, false],
]) {
  const enc = new Connection(shareAim);
  let dec = new Connection();
  let bytes = 0,
    keys = 0,
    ns = 0n,
    snapBytes = 0;
  for (const [i, f] of frames.entries()) {
    const key = !i || i % interval === 0 || f.cut != null;
    if (i === 4771) {
      // A different receiver joins immediately before this delta.
      const snap = enc.snapshot();
      dec = new Connection();
      dec.resume(snap);
      snapBytes = snap.length;
    }
    const start = process.hrtime.bigint();
    const packet = enc.encode(f, key);
    ns += process.hrtime.bigint() - start;
    bytes += packet.byteLength;
    keys += +key;
    assert.deepStrictEqual(dec.decode(packet), f, `frame ${i}`);
  }
  console.log(
    JSON.stringify({
      key_seconds: Number.isFinite(interval) ? interval / 15 : null,
      cross_player_aim: shareAim,
      frames: frames.length,
      keys,
      wire_bytes: bytes,
      bytes_per_player_second: bytes / 9600,
      encode_us_per_frame: Number(ns) / 1e3 / frames.length,
      out_of_band_snapshot_bytes_at_frame_4771: snapBytes,
    }),
  );
}
