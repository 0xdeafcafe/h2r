// A save's own SaveImage.xbx (or the title's), decoded in the browser: an
// `XPR0` resource holding one texture, the format crates/platform/assets/src/xpr.rs
// reads from the disc's executable. Saves are small (64x64) and DXT1 in
// practice; DXT1, DXT3 and DXT5 are read, anything else is null and the
// caller falls back to the disc's save image.
//
//   +0 'XPR0'  +4 total size  +8 header size (where the data starts)
//   +12 Common (type bits 0x00070000 == 0x00040000 for a texture)
//   +16 Data (offset from the data start)  +20 Lock
//   +24 Format: 27..24 log2 height, 23..20 log2 width, 15..8 D3D format
//   +28 Size (0 for a block-compressed texture)

export interface Rgba { width: number; height: number; data: Uint8ClampedArray }

const DXT1 = 0x0c, DXT3 = 0x0e, DXT5 = 0x0f;
const MAX_SIDE = 1024;

const u16 = (b: Uint8Array, o: number) => b[o]! | (b[o + 1]! << 8);
const u32 = (b: Uint8Array, o: number) => (b[o]! | (b[o + 1]! << 8) | (b[o + 2]! << 16) | (b[o + 3]! << 24)) >>> 0;

function rgb565(c: number): [number, number, number] {
  const r = (c >> 11) & 31, g = (c >> 5) & 63, b = c & 31;
  return [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)];
}

/** The four colours of a block's colour half; `alpha1` for DXT1's
 *  punch-through mode (c0 <= c1). */
function palette(b: Uint8Array, o: number, dxt1: boolean): number[][] {
  const c0 = u16(b, o), c1 = u16(b, o + 2);
  const p0 = rgb565(c0), p1 = rgb565(c1);
  const mix = (a: number, x: number, y: number, d: number) => [0, 1, 2].map((i) => Math.round((a * p0[i]! + x * p1[i]!) / d)).concat([y]);
  if (!dxt1 || c0 > c1) return [[...p0, 255], [...p1, 255], mix(2, 1, 255, 3), mix(1, 2, 255, 3)];
  return [[...p0, 255], [...p1, 255], mix(1, 1, 255, 2), [0, 0, 0, 0]];
}

function dxt5Alpha(b: Uint8Array, o: number): number[] {
  const a0 = b[o]!, a1 = b[o + 1]!;
  const a = [a0, a1];
  if (a0 > a1) for (let i = 1; i < 7; i++) a.push(Math.round(((7 - i) * a0 + i * a1) / 7));
  else { for (let i = 1; i < 5; i++) a.push(Math.round(((5 - i) * a0 + i * a1) / 5)); a.push(0, 255); }
  const out: number[] = [];
  // 16 3-bit indices, little-endian over six bytes.
  let bits = 0n;
  for (let i = 5; i >= 0; i--) bits = (bits << 8n) | BigInt(b[o + 2 + i]!);
  for (let i = 0; i < 16; i++) out.push(a[Number((bits >> BigInt(3 * i)) & 7n)]!);
  return out;
}

/** Decodes the texture of an `XPR0` resource, or null for anything this
 *  doesn't read. */
export function decodeXpr(bytes: Uint8Array): Rgba | null {
  if (bytes.length < 32 || String.fromCharCode(...bytes.subarray(0, 4)) !== "XPR0") return null;
  const total = Math.min(u32(bytes, 4), bytes.length), header = u32(bytes, 8);
  if (header < 32 || header > total) return null;
  if ((u32(bytes, 12) & 0x00070000) !== 0x00040000) return null;
  const format = u32(bytes, 24);
  if (u32(bytes, 28) !== 0) return null;
  const fmt = (format >> 8) & 0xff;
  if (fmt !== DXT1 && fmt !== DXT3 && fmt !== DXT5) return null;
  const width = 1 << ((format >>> 20) & 0xf), height = 1 << ((format >>> 24) & 0xf);
  if (width > MAX_SIDE || height > MAX_SIDE) return null;
  const blockBytes = fmt === DXT1 ? 8 : 16;
  const bw = Math.ceil(width / 4), bh = Math.ceil(height / 4);
  const start = header + u32(bytes, 16);
  if (start + bw * bh * blockBytes > total) return null;
  const data = new Uint8ClampedArray(width * height * 4);
  for (let by = 0; by < bh; by++) for (let bx = 0; bx < bw; bx++) {
    const o = start + (by * bw + bx) * blockBytes;
    const colour = fmt === DXT1 ? o : o + 8;
    const pal = palette(bytes, colour, fmt === DXT1);
    const idx = u32(bytes, colour + 4);
    const alpha = fmt === DXT5 ? dxt5Alpha(bytes, o) : null;
    for (let i = 0; i < 16; i++) {
      const x = bx * 4 + (i & 3), y = by * 4 + (i >> 2);
      if (x >= width || y >= height) continue;
      const p = pal[(idx >>> (2 * i)) & 3]!;
      const d = (y * width + x) * 4;
      data[d] = p[0]!; data[d + 1] = p[1]!; data[d + 2] = p[2]!;
      data[d + 3] = fmt === DXT3 ? ((bytes[o + (i >> 1)]! >> ((i & 1) * 4)) & 15) * 17 : alpha ? alpha[i]! : p[3]!;
    }
  }
  return { width, height, data };
}
