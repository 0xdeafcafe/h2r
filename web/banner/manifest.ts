// Adding the banners to the art's manifest.json (banner/run.ts). Pure, so
// it can be tested.

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
