// cd web && bun bench/archive-transfer.ts <H2_ARCHIVE_DUMP>
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
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
  let count = 0;
  for (const [i, file] of files.entries()) {
    const expected = JSON.parse(readFileSync(join(dir, file.replace(/\.br$/, ".json")), "utf8"));
    const actual = decodeArchive(brotliDecompressSync(buffers[i]!));
    equal(actual, expected);
    count += actual.length;
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
  console.log(JSON.stringify({scene:prefix, frames:count, segments:files.length,
    bytes:buffers.reduce((n,b) => n+b.length,0), first_bytes:buffers[0]!.length,
    first_decode_ms:median(firstMs), full_decode_ms:median(ms)}));
}
