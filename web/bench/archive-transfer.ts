// cd web && bun bench/archive-transfer.ts <H2_ARCHIVE_DUMP>
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { brotliDecompressSync } from "node:zlib";
import { decodeArchive } from "../src/api/archive";

const dir = process.argv[2];
if (!dir) throw new Error("pass the bench_archive_scenes dump directory");
function equal(a: unknown, b: unknown): void {
  if (Object.is(a, b)) return;
  if (!a || !b || typeof a !== "object" || typeof b !== "object") assert.fail(`${JSON.stringify(a)} versus ${JSON.stringify(b)}`);
  assert.equal(Array.isArray(a), Array.isArray(b));
  const keys = Object.keys(b);
  assert.equal(keys.length, Object.keys(a).length);
  for (const k of keys) equal((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]);
}
for (const prefix of ["mp16", "campaign"]) {
  const files = readdirSync(dir).filter(n => n.startsWith(prefix + "-") && n.endsWith(".br")).sort();
  if (!files.length) continue;
  const buffers = files.map(n => readFileSync(join(dir, n)));
  let count = 0, indexBytes = 0, playerSamples = 0;
  let previousHash: string | null = null;
  const sha = (b: Uint8Array) => createHash("sha256").update(b).digest("hex");
  // Each run uses the same two-digit signed source-log position. This
  // reproduces the exact immutable node bytes in replay_archive/job.rs.
  const revision = 64;
  for (const [i, file] of files.entries()) {
    const expected = JSON.parse(readFileSync(join(dir, file.replace(/\.br$/, ".json")), "utf8"));
    const actual = decodeArchive(brotliDecompressSync(buffers[i]!));
    equal(actual, expected);
    count += actual.length;
    const keyframes: number[] = [];
    for (const [n, frame] of actual.entries()) {
      playerSamples += frame.players.length;
      // Both seeded scenes start each H2CK on a 300-tick boundary. The
      // archive scene test checks the source key bits against these offsets.
      if (Number(frame.tick) % 300 === 0) keyframes.push(n);
    }
    const first = actual[0]!, last = actual.at(-1)!;
    const segment = {from_tick:first.tick, to_tick:last.tick, frames:actual.length,
      bytes:buffers[i]!.byteLength, sha256:sha(buffers[i]!), keyframes};
    const node = Buffer.from(JSON.stringify(["H2 replay archive v1", first.game_id, revision, previousHash, segment]));
    indexBytes += node.byteLength;
    previousHash = sha(node);
  }
  const ms: number[] = [];
  const firstMs: number[] = [];
  for (let run = 0; run < 6; run++) {
    Bun.gc(true);
    const start = performance.now();
    let n = 0;
    for (const [i, bytes] of buffers.entries()) {
      n += decodeArchive(brotliDecompressSync(bytes)).length;
      if (run && !i) firstMs.push(performance.now() - start);
    }
    if (run) ms.push(performance.now() - start);
    assert.equal(n, count);
  }
  const median = (a: number[]) => a.sort((x, y) => x-y)[a.length>>1];
  const payload = buffers.reduce((total, b) => total + b.length, 0);
  // Guard the concrete 10-minute multiplayer and 30-second campaign runs,
  // including immutable index node overhead, against their stored v7 inputs.
  const source = prefix === "mp16" ? 1322664 : 173708;
  assert.equal(count, prefix === "mp16" ? 9000 : 450);
  assert.ok((payload + indexBytes) * 4 <= source, `${prefix} exceeds a quarter of its stored v7 size`);
  console.log(JSON.stringify({scene:prefix, frames:count, segments:files.length,
    stored_v7_bytes:source, fraction_of_stored_v7:(payload + indexBytes) / source,
    bytes:payload, index_bytes:indexBytes, node_source_position:revision,
    bytes_per_player_minute:(payload + indexBytes) / (playerSamples / (15 * 60)),
    first_bytes:buffers[0]!.length,
    first_decode_ms:median(firstMs), full_decode_ms:median(ms)}));
}
