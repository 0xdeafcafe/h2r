// The original browser compositor, retained for synthetic mock art and
// as the independent pixel reference for live-server's Rust compositor.

import { EMBLEM_PATH } from "../src/lib/emblem";
import { emblemPalette, parseHexColour, type EmblemSpec, type GameArtManifest } from "../src/lib/gameart";

/**
 * Colours an emblem from its two layer images (RGBA bytes, same size).
 *
 * A layer holds two masks, not colours: mask a is the red channel, mask b
 * the blue one. The image's alpha copies mask b and is not coverage, so it
 * is ignored. Per pixel, channels in 0..1:
 *
 *   rgb      = colour_a * red + colour_b * blue
 *   coverage = background: 1;  foreground: max(red, blue)
 *
 * The foreground's rgb is already weighted by its masks, so it goes over
 * the background as `fg + bg * (1 - coverage)`. The result is opaque.
 */
export function compositeEmblem(
  bg: ArrayLike<number>, fg: ArrayLike<number>,
  colours: Pick<EmblemSpec, "primary" | "secondary" | "background" | "background2">,
  out: Uint8ClampedArray = new Uint8ClampedArray(bg.length),
): Uint8ClampedArray {
  const { primary: fa, secondary: fb, background: ba, background2: bb } = colours;
  for (let i = 0; i + 3 < bg.length; i += 4) {
    const br = bg[i]! / 255, bbm = bg[i + 2]! / 255;
    const fr = (fg[i] ?? 0) / 255, fbm = (fg[i + 2] ?? 0) / 255;
    const keep = 1 - Math.max(fr, fbm);
    for (let c = 0; c < 3; c++) {
      const under = Math.min(255, ba[c]! * br + bb[c]! * bbm);
      out[i + c] = Math.round(fa[c]! * fr + fb[c]! * fbm + under * keep);
    }
    out[i + 3] = 255;
  }
  return out;
}

/** The mock draws all 64 foreground and 32 background masks. */
export function mockEmblemSpec(path: string, manifest: GameArtManifest): EmblemSpec | null {
  if (!path.startsWith(EMBLEM_PATH) || !path.endsWith(".png")) return null;
  const [fg, bg, primary, secondary, background, background2, seed, ...extra] =
    path.slice(EMBLEM_PATH.length, -4).split("-");
  const index = (s: string | undefined, max: number) =>
    s && /^(0|[1-9][0-9]{0,2})$/.test(s) && Number(s) < max ? Number(s) : undefined;
  const hash = (s: string | undefined) => s && /^s[0-9a-f]{8}$/.test(s) ? Number.parseInt(s.slice(1), 16) : undefined;
  const layer = (s: string | undefined, n: number) => index(s, n) ?? ((hash(s) ?? NaN) % n);
  const f = layer(fg, 64), b = layer(bg, 32);
  if (!Number.isInteger(f) || !Number.isInteger(b) || extra.length) return null;

  const p = hash(primary), s = hash(secondary), ba = hash(background), bb = hash(background2);
  const needsSeed = (p === undefined && [s, ba, bb].some((i) => i !== undefined)) || (ba === undefined && bb !== undefined);
  if (needsSeed ? !seed || !/^[0-9a-f]{16}$/.test(seed) : seed !== undefined) return null;
  const palette = emblemPalette(manifest), n = palette.length;
  const fallback = (start: number) => seed ? Number.parseInt(seed.slice(start, start + 8), 16) : 0;
  const seededPrimary = (p ?? fallback(0)) % n;
  const step = (from: number, h: number) => (from + 1 + h % (n - 1)) % n;
  const seededBackground = step(seededPrimary, ba ?? fallback(8));
  const colour = (text: string | undefined, defaultIndex: number) => {
    if (text && /^r[0-9a-f]{6}$/.test(text)) return parseHexColour(text.slice(1));
    const i = index(text, 256);
    if (i !== undefined) return palette[i % n];
    if (hash(text) !== undefined) return palette[defaultIndex];
    return null;
  };
  const pa = colour(primary, seededPrimary), pb = colour(secondary, step(seededPrimary, s ?? 0));
  const ca = colour(background, seededBackground), cb = colour(background2, step(seededBackground, bb ?? 0));
  return pa && pb && ca && cb ? { fg: f, bg: b, primary: pa, secondary: pb, background: ca, background2: cb } : null;
}
