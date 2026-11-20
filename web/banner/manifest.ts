// Adding the banners to the art's manifest.json (banner/run.ts), and
// writing it back with fresh compressed copies. Kept apart from run.ts so it
// can be tested.

import { renameSync, writeFileSync } from "node:fs";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

export interface Manifest { images?: Record<string, unknown>; [k: string]: unknown }

export const BANNER_PREFIX = "banner/";

/** The manifest with its `banner/*` images replaced by `banners`: the ones
 *  drawn now, and no stale ones from an earlier run. Everything else is
 *  kept as it was. */
export function applyBanners(m: Manifest, banners: Array<{ id: string; file: string; width: number; height: number }>): Manifest {
  const images: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(m.images ?? {})) if (!k.startsWith(BANNER_PREFIX)) images[k] = v;
  for (const b of banners) images[BANNER_PREFIX + b.id] = { file: b.file, width: b.width, height: b.height };
  return { ...m, images };
}

/** `bytes` as the extractor's packing writes a copy (crates/live/live-server/src/
 *  art_pack.rs `brotli` and `gzip`): Brotli quality 11 with a 16 MiB window,
 *  gzip level 9. */
export function compressedCopies(bytes: Uint8Array): { br: Buffer; gz: Buffer } {
  return {
    br: brotliCompressSync(bytes, {
      params: {
        [constants.BROTLI_PARAM_QUALITY]: 11,
        [constants.BROTLI_PARAM_LGWIN]: 24,
        [constants.BROTLI_PARAM_SIZE_HINT]: bytes.length,
      },
    }),
    gz: gzipSync(bytes, { level: 9 }),
  };
}

/** Writes `m` to `path` (the art's manifest.json) and then its `.br` and
 *  `.gz` copies from the same bytes, each through a temporary file and a
 *  rename. The copies must be rewritten: the server and `h2 live assets
 *  publish` send the Brotli copy in place of the file, so a copy left from
 *  the extraction would hide the banners (docs/live/live-art.md, "Packing").
 *  Written after the file, the copies are never older than it. */
export function writeManifest(path: string, m: Manifest): void {
  const bytes = Buffer.from(JSON.stringify(m, null, 2) + "\n");
  const { br, gz } = compressedCopies(bytes);
  for (const [to, data] of [[path, bytes], [path + ".br", br], [path + ".gz", gz]] as const) {
    const tmp = to + ".banner";
    writeFileSync(tmp, data);
    renameSync(tmp, to);
  }
}
