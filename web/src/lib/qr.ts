// A small QR code encoder (ISO/IEC 18004), for the otpauth:// URI on the
// recovery screens: byte mode only, error correction L or M, versions 1
// to 10 (up to 213 bytes at M), which is plenty for a URI of ~120
// characters. The structure follows the spec's steps: data bits, Reed-
// Solomon blocks interleaved, function patterns, the zigzag placement and
// the mask with the lowest penalty score.

export type Ecl = "L" | "M";

export const MAX_VERSION = 10;

// Per version (index 1..10): error correction codewords per block and the
// number of blocks (ISO/IEC 18004 table 9).
const ECC_PER_BLOCK: Record<Ecl, readonly number[]> = {
  L: [0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18],
  M: [0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26],
};
const BLOCKS: Record<Ecl, readonly number[]> = {
  L: [0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4],
  M: [0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5],
};
// Alignment pattern centres (annex E).
const ALIGN: readonly (readonly number[])[] = [
  [], [], [6, 18], [6, 22], [6, 26], [6, 30], [6, 34], [6, 22, 38], [6, 24, 42], [6, 26, 46], [6, 28, 50],
];
// The two format bits for each level: L is 01, M is 00.
const ECL_BITS: Record<Ecl, number> = { L: 1, M: 0 };

export interface Qr {
  version: number;
  ecl: Ecl;
  mask: number;
  size: number;
  /** modules[y][x], true is dark. */
  modules: boolean[][];
}

/** Modules left for data and error correction once the function patterns are drawn. */
function rawModules(v: number): number {
  let n = (16 * v + 128) * v + 64;
  if (v >= 2) {
    const align = Math.floor(v / 7) + 2;
    n -= (25 * align - 10) * align - 55;
    if (v >= 7) n -= 36;
  }
  return n;
}

export function totalCodewords(v: number): number {
  return Math.floor(rawModules(v) / 8);
}

export function dataCodewords(v: number, ecl: Ecl): number {
  return totalCodewords(v) - ECC_PER_BLOCK[ecl][v]! * BLOCKS[ecl][v]!;
}

// ---------- Reed-Solomon over GF(256), polynomial 0x11D ----------

export function gfMul(x: number, y: number): number {
  let z = 0;
  for (let i = 7; i >= 0; i--) {
    z = (z << 1) ^ ((z >>> 7) * 0x11d);
    z ^= ((y >>> i) & 1) * x;
  }
  return z & 0xff;
}

/** The generator polynomial of `degree`, highest term first, its leading 1 left out. */
export function rsDivisor(degree: number): number[] {
  const out = new Array<number>(degree).fill(0);
  out[degree - 1] = 1;
  let root = 1;
  for (let i = 0; i < degree; i++) {
    for (let j = 0; j < degree; j++) {
      out[j] = gfMul(out[j]!, root);
      if (j + 1 < degree) out[j] = out[j]! ^ out[j + 1]!;
    }
    root = gfMul(root, 2);
  }
  return out;
}

/** The error correction codewords for `data`. */
export function rsRemainder(data: readonly number[], divisor: readonly number[]): number[] {
  const out = new Array<number>(divisor.length).fill(0);
  for (const b of data) {
    const factor = b ^ out.shift()!;
    out.push(0);
    divisor.forEach((c, i) => { out[i]! ^= gfMul(c, factor); });
  }
  return out;
}

// ---------- format and version information (BCH codes) ----------

/** The 15 format bits for a level and mask, masked with 101010000010010. */
export function formatBits(ecl: Ecl, mask: number): number {
  const data = (ECL_BITS[ecl] << 3) | mask;
  let rem = data;
  for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
  return ((data << 10) | rem) ^ 0x5412;
}

/** The 18 version bits (versions 7 and up). */
export function versionBits(v: number): number {
  let rem = v;
  for (let i = 0; i < 12; i++) rem = (rem << 1) ^ ((rem >>> 11) * 0x1f25);
  return (v << 12) | rem;
}

// ---------- encoding ----------

/** The data codewords for `bytes` in byte mode, padded to version `v`. */
export function dataCodewordsFor(bytes: Uint8Array, v: number, ecl: Ecl): number[] {
  const cap = dataCodewords(v, ecl) * 8;
  const bits: number[] = [];
  const put = (val: number, len: number) => { for (let i = len - 1; i >= 0; i--) bits.push((val >>> i) & 1); };
  put(0b0100, 4);
  put(bytes.length, v <= 9 ? 8 : 16);
  for (const b of bytes) put(b, 8);
  if (bits.length > cap) throw new Error("too long for this version");
  put(0, Math.min(4, cap - bits.length));
  put(0, (8 - (bits.length % 8)) % 8);
  const out: number[] = [];
  for (let i = 0; i < bits.length; i += 8) out.push(bits.slice(i, i + 8).reduce((a, b) => (a << 1) | b, 0));
  for (let pad = 0xec; out.length < cap / 8; pad ^= 0xec ^ 0x11) out.push(pad);
  return out;
}

/** The final codeword sequence: data split into blocks, each with its
 *  error correction, interleaved. */
export function interleave(data: readonly number[], v: number, ecl: Ecl): number[] {
  const nBlocks = BLOCKS[ecl][v]!, ecc = ECC_PER_BLOCK[ecl][v]!;
  const raw = totalCodewords(v);
  const nShort = nBlocks - (raw % nBlocks);
  const shortData = Math.floor(raw / nBlocks) - ecc;
  const divisor = rsDivisor(ecc);
  const blocks: { data: number[]; ecc: number[] }[] = [];
  for (let i = 0, k = 0; i < nBlocks; i++) {
    const len = shortData + (i < nShort ? 0 : 1);
    const d = data.slice(k, k + len);
    k += len;
    blocks.push({ data: d, ecc: rsRemainder(d, divisor) });
  }
  const out: number[] = [];
  for (let i = 0; i <= shortData; i++) for (const b of blocks) if (i < b.data.length) out.push(b.data[i]!);
  for (let i = 0; i < ecc; i++) for (const b of blocks) out.push(b.ecc[i]!);
  return out;
}

const MASKS: ReadonlyArray<(x: number, y: number) => boolean> = [
  (x, y) => (x + y) % 2 === 0,
  (_x, y) => y % 2 === 0,
  (x) => x % 3 === 0,
  (x, y) => (x + y) % 3 === 0,
  (x, y) => (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0,
  (x, y) => ((x * y) % 2) + ((x * y) % 3) === 0,
  (x, y) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
  (x, y) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
];

class Grid {
  readonly size: number;
  readonly m: boolean[][];
  readonly fn: boolean[][];
  constructor(readonly version: number) {
    this.size = version * 4 + 17;
    this.m = Array.from({ length: this.size }, () => new Array<boolean>(this.size).fill(false));
    this.fn = Array.from({ length: this.size }, () => new Array<boolean>(this.size).fill(false));
  }
  set(x: number, y: number, dark: boolean) {
    this.m[y]![x] = dark;
    this.fn[y]![x] = true;
  }
  drawFunctionPatterns(ecl: Ecl) {
    const n = this.size;
    for (let i = 0; i < n; i++) { this.set(6, i, i % 2 === 0); this.set(i, 6, i % 2 === 0); }
    for (const [cx, cy] of [[3, 3], [n - 4, 3], [3, n - 4]] as const) {
      for (let dy = -4; dy <= 4; dy++) for (let dx = -4; dx <= 4; dx++) {
        const x = cx + dx, y = cy + dy;
        if (x < 0 || y < 0 || x >= n || y >= n) continue;
        const d = Math.max(Math.abs(dx), Math.abs(dy));
        this.set(x, y, d !== 2 && d !== 4);
      }
    }
    const al = ALIGN[this.version]!, last = al.length - 1;
    al.forEach((ax, i) => al.forEach((ay, j) => {
      if ((i === 0 && j === 0) || (i === 0 && j === last) || (i === last && j === 0)) return;
      for (let dy = -2; dy <= 2; dy++) for (let dx = -2; dx <= 2; dx++) this.set(ax + dx, ay + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
    }));
    this.drawFormat(ecl, 0);
    if (this.version >= 7) {
      const bits = versionBits(this.version);
      for (let i = 0; i < 18; i++) {
        const dark = ((bits >>> i) & 1) === 1;
        const a = n - 11 + (i % 3), b = Math.floor(i / 3);
        this.set(a, b, dark);
        this.set(b, a, dark);
      }
    }
  }
  drawFormat(ecl: Ecl, mask: number) {
    const n = this.size, bits = formatBits(ecl, mask);
    const bit = (i: number) => ((bits >>> i) & 1) === 1;
    for (let i = 0; i <= 5; i++) this.set(8, i, bit(i));
    this.set(8, 7, bit(6));
    this.set(8, 8, bit(7));
    this.set(7, 8, bit(8));
    for (let i = 9; i < 15; i++) this.set(14 - i, 8, bit(i));
    for (let i = 0; i < 8; i++) this.set(n - 1 - i, 8, bit(i));
    for (let i = 8; i < 15; i++) this.set(8, n - 15 + i, bit(i));
    this.set(8, n - 8, true);
  }
  drawCodewords(cw: readonly number[]) {
    const n = this.size;
    let i = 0;
    for (let right = n - 1; right >= 1; right -= 2) {
      if (right === 6) right = 5;
      for (let vert = 0; vert < n; vert++) {
        for (let j = 0; j < 2; j++) {
          const x = right - j;
          const up = ((right + 1) & 2) === 0;
          const y = up ? n - 1 - vert : vert;
          if (!this.fn[y]![x] && i < cw.length * 8) {
            this.m[y]![x] = ((cw[i >>> 3]! >>> (7 - (i & 7))) & 1) === 1;
            i++;
          }
        }
      }
    }
  }
  applyMask(mask: number) {
    const f = MASKS[mask]!;
    for (let y = 0; y < this.size; y++) for (let x = 0; x < this.size; x++) {
      if (!this.fn[y]![x] && f(x, y)) this.m[y]![x] = !this.m[y]![x];
    }
  }
}

const FINDER_LIKE = [[true, false, true, true, true, false, true, false, false, false, false], [false, false, false, false, true, false, true, true, true, false, true]];

/** The spec's penalty score (rules N1 to N4); lower scans better. */
export function penalty(m: readonly (readonly boolean[])[]): number {
  const n = m.length;
  let score = 0, dark = 0;
  const line = (get: (i: number) => boolean) => {
    let run = 1;
    for (let i = 1; i <= n; i++) {
      if (i < n && get(i) === get(i - 1)) run++;
      else { if (run >= 5) score += 3 + run - 5; run = 1; }
    }
    for (let i = 0; i + 11 <= n; i++) {
      for (const p of FINDER_LIKE) if (p.every((v, k) => get(i + k) === v)) score += 40;
    }
  };
  for (let y = 0; y < n; y++) line((i) => m[y]![i]!);
  for (let x = 0; x < n; x++) line((i) => m[i]![x]!);
  for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) {
    if (m[y]![x]) dark++;
    if (x + 1 < n && y + 1 < n) {
      const c = m[y]![x];
      if (m[y]![x + 1] === c && m[y + 1]![x] === c && m[y + 1]![x + 1] === c) score += 3;
    }
  }
  const total = n * n;
  score += Math.floor(Math.abs(dark * 20 - total * 10) / total) * 10;
  return score;
}

/** Encodes `text` (UTF-8, byte mode) at the smallest version that fits.
 *  `mask` forces a mask (0..7); by default the lowest-penalty one. */
export function qrEncode(text: string, ecl: Ecl = "M", opts: { mask?: number; minVersion?: number } = {}): Qr {
  const bytes = new TextEncoder().encode(text);
  let version = 0;
  for (let v = Math.max(1, opts.minVersion ?? 1); v <= MAX_VERSION; v++) {
    if (4 + (v <= 9 ? 8 : 16) + bytes.length * 8 <= dataCodewords(v, ecl) * 8) { version = v; break; }
  }
  if (!version) throw new Error(`too long for a QR code up to version ${MAX_VERSION} (${bytes.length} bytes)`);
  const cw = interleave(dataCodewordsFor(bytes, version, ecl), version, ecl);
  const build = (mask: number) => {
    const g = new Grid(version);
    g.drawFunctionPatterns(ecl);
    g.drawCodewords(cw);
    g.applyMask(mask);
    g.drawFormat(ecl, mask);
    return g;
  };
  let best: Grid | null = null, bestMask = 0, bestScore = Infinity;
  for (let mask = 0; mask < 8; mask++) {
    if (opts.mask !== undefined && opts.mask !== mask) continue;
    const g = build(mask);
    const s = opts.mask !== undefined ? 0 : penalty(g.m);
    if (s < bestScore) { best = g; bestMask = mask; bestScore = s; }
  }
  return { version, ecl, mask: bestMask, size: best!.size, modules: best!.m };
}

/** An SVG path of the dark modules, one unit per module, offset by `border`. */
export function qrPath(qr: Qr, border = 4): string {
  let d = "";
  for (let y = 0; y < qr.size; y++) {
    for (let x = 0; x < qr.size; x++) {
      if (!qr.modules[y]![x]) continue;
      let w = 1;
      while (x + w < qr.size && qr.modules[y]![x + w]) w++;
      d += `M${x + border} ${y + border}h${w}v1h-${w}z`;
      x += w - 1;
    }
  }
  return d;
}
