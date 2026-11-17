import { describe, expect, test } from "bun:test";
import { dataCodewords, formatBits, gfMul, qrEncode, qrPath, rsDivisor, rsRemainder, totalCodewords, versionBits, type Ecl, type Qr } from "../src/lib/qr";

// A decoder written from the spec, sharing nothing with the encoder but
// GF(256) multiplication: function pattern map, format bits, zigzag read,
// unmask, de-interleave, Reed-Solomon syndromes, byte-mode parse.

const SPEC_BLOCKS: Record<Ecl, Record<number, [ecc: number, groups: Array<[count: number, data: number]>]>> = {
  // ISO/IEC 18004 table 9, as printed (block count x data codewords).
  L: { 1: [7, [[1, 19]]], 2: [10, [[1, 34]]], 3: [15, [[1, 55]]], 4: [20, [[1, 80]]], 5: [26, [[1, 108]]], 6: [18, [[2, 68]]], 7: [20, [[2, 78]]], 8: [24, [[2, 97]]], 9: [30, [[2, 116]]], 10: [18, [[2, 68], [2, 69]]] },
  M: { 1: [10, [[1, 16]]], 2: [16, [[1, 28]]], 3: [26, [[1, 44]]], 4: [18, [[2, 32]]], 5: [24, [[2, 43]]], 6: [16, [[4, 27]]], 7: [18, [[4, 31]]], 8: [22, [[2, 38], [2, 39]]], 9: [22, [[3, 36], [2, 37]]], 10: [26, [[4, 43], [1, 44]]] },
};

function alignCentres(v: number): number[] {
  if (v === 1) return [];
  const n = Math.floor(v / 7) + 2, size = v * 4 + 17;
  const step = Math.ceil((v * 4 + 4) / (n * 2 - 2)) * 2;
  const out = [6];
  for (let p = size - 7; out.length < n; p -= step) out.splice(1, 0, p);
  return out;
}

function functionMap(v: number): boolean[][] {
  const n = v * 4 + 17;
  const f = Array.from({ length: n }, () => new Array<boolean>(n).fill(false));
  const box = (x0: number, y0: number, w: number, h: number) => { for (let y = y0; y < y0 + h; y++) for (let x = x0; x < x0 + w; x++) f[y]![x] = true; };
  box(0, 0, 9, 9); box(n - 8, 0, 8, 9); box(0, n - 8, 9, 8);
  box(6, 0, 1, n); box(0, 6, n, 1);
  const a = alignCentres(v);
  // Every centre pair but the three that would sit on a finder.
  for (const x of a) for (const y of a) if (!(x === 6 && y === 6) && !(x === 6 && y === n - 7) && !(x === n - 7 && y === 6)) box(x - 2, y - 2, 5, 5);
  if (v >= 7) { box(n - 11, 0, 3, 6); box(0, n - 11, 6, 3); }
  return f;
}

function decode(qr: Qr): { text: string; ecl: Ecl; mask: number } {
  const m = qr.modules, n = m.length, v = (n - 17) / 4;
  // Format: bits 14..0 read along the top-left copy.
  const coords: Array<[number, number]> = [];
  for (let i = 0; i <= 5; i++) coords.push([8, i]);
  coords.push([8, 7], [8, 8], [7, 8]);
  for (let i = 9; i < 15; i++) coords.push([14 - i, 8]);
  let fmt = 0;
  coords.forEach(([x, y], i) => { if (m[y]![x]) fmt |= 1 << i; });
  fmt ^= 0x5412;
  const ecl: Ecl = ((fmt >>> 13) & 3) === 1 ? "L" : "M";
  const mask = (fmt >>> 10) & 7;
  const fn = functionMap(v);
  const masks = [
    (x: number, y: number) => (x + y) % 2 === 0, (_: number, y: number) => y % 2 === 0, (x: number) => x % 3 === 0,
    (x: number, y: number) => (x + y) % 3 === 0, (x: number, y: number) => (Math.floor(y / 2) + Math.floor(x / 3)) % 2 === 0,
    (x: number, y: number) => ((x * y) % 2) + ((x * y) % 3) === 0, (x: number, y: number) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
    (x: number, y: number) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
  ];
  const bits: number[] = [];
  let upward = true;
  for (let col = n - 1; col > 0; col -= 2) {
    if (col === 6) col--;
    for (let k = 0; k < n; k++) {
      const y = upward ? n - 1 - k : k;
      for (const x of [col, col - 1]) if (!fn[y]![x]) bits.push((m[y]![x] ? 1 : 0) ^ (masks[mask]!(x, y) ? 1 : 0));
    }
    upward = !upward;
  }
  const bytes: number[] = [];
  for (let i = 0; i + 8 <= bits.length; i += 8) bytes.push(bits.slice(i, i + 8).reduce((a, b) => (a << 1) | b, 0));
  const [ecc, groups] = SPEC_BLOCKS[ecl][v]!;
  const lens = groups.flatMap(([c, d]) => new Array<number>(c).fill(d));
  const blocks = lens.map(() => [] as number[]);
  let k = 0;
  for (let i = 0; i < Math.max(...lens); i++) lens.forEach((l, b) => { if (i < l) blocks[b]!.push(bytes[k++]!); });
  for (let i = 0; i < ecc; i++) blocks.forEach((b) => b.push(bytes[k++]!));
  // Every block is a codeword: its syndromes at α^0..α^(ecc-1) are zero.
  for (const b of blocks) {
    let alpha = 1;
    for (let j = 0; j < ecc; j++) {
      let s = 0;
      for (const c of b) s = gfMul(s, alpha) ^ c;
      expect(s).toBe(0);
      alpha = gfMul(alpha, 2);
    }
  }
  const data = blocks.flatMap((b, i) => b.slice(0, lens[i]));
  const dbits = data.flatMap((b) => [7, 6, 5, 4, 3, 2, 1, 0].map((s) => (b >>> s) & 1));
  let p = 0;
  const take = (len: number) => { let x = 0; for (let i = 0; i < len; i++) x = (x << 1) | dbits[p++]!; return x; };
  expect(take(4)).toBe(0b0100);
  const count = take(v <= 9 ? 8 : 16);
  const out = new Uint8Array(count);
  for (let i = 0; i < count; i++) out[i] = take(8);
  return { text: new TextDecoder().decode(out), ecl, mask };
}

describe("qr encoder", () => {
  test("capacities match the spec's table", () => {
    expect([1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map(totalCodewords)).toEqual([26, 44, 70, 100, 134, 172, 196, 242, 292, 346]);
    for (const ecl of ["L", "M"] as const) {
      for (const [v, [, groups]] of Object.entries(SPEC_BLOCKS[ecl])) {
        expect(dataCodewords(Number(v), ecl)).toBe(groups.reduce((a, [c, d]) => a + c * d, 0));
      }
    }
  });

  test("Reed-Solomon: the HELLO WORLD 1-M example's error correction", () => {
    const data = [0x20, 0x5b, 0x0b, 0x78, 0xd1, 0x72, 0xdc, 0x4d, 0x43, 0x40, 0xec, 0x11, 0xec, 0x11, 0xec, 0x11];
    expect(rsRemainder(data, rsDivisor(10))).toEqual([196, 35, 39, 119, 235, 215, 231, 226, 93, 23]);
  });

  test("format and version information vectors", () => {
    expect(formatBits("M", 0).toString(2).padStart(15, "0")).toBe("101010000010010");
    expect(formatBits("L", 0).toString(2).padStart(15, "0")).toBe("111011111000100");
    expect(formatBits("L", 4).toString(2).padStart(15, "0")).toBe("110011000101111");
    expect(formatBits("M", 5).toString(2).padStart(15, "0")).toBe("100000011001110");
    expect(formatBits("M", 7).toString(2).padStart(15, "0")).toBe("100101010100000");
    expect(versionBits(7)).toBe(0x07c94);
    expect(versionBits(10)).toBe(0x0a4d3);
  });

  test("picks the smallest version that fits", () => {
    expect(qrEncode("hello").version).toBe(1);
    expect(qrEncode("a".repeat(14)).version).toBe(1); // 14 bytes is 1-M's limit
    expect(qrEncode("a".repeat(15)).version).toBe(2);
    expect(qrEncode("a".repeat(213)).version).toBe(10);
    expect(() => qrEncode("a".repeat(214))).toThrow("too long");
    expect(qrEncode("a".repeat(17), "L").version).toBe(1);
  });

  const URI = "otpauth://totp/h2%20live%20(live.example.net):Sneaky%20Pete?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=h2%20live&period=30&digits=6&algorithm=SHA1";

  test("an otpauth URI round-trips through the decoder at every mask", () => {
    for (let mask = 0; mask < 8; mask++) {
      const qr = qrEncode(URI, "M", { mask });
      expect(qr.size).toBe(qr.version * 4 + 17);
      const d = decode(qr);
      expect(d).toEqual({ text: URI, ecl: "M", mask });
    }
  });

  test("round trips across versions 1 to 10, L and M, including UTF-8", () => {
    for (const ecl of ["L", "M"] as const) {
      for (let v = 1; v <= 10; v++) {
        const text = "Ünïcode ✓ " + "x".repeat(Math.max(0, dataCodewords(v, ecl) - 20));
        const qr = qrEncode(text, ecl, { minVersion: v });
        expect(qr.version).toBe(v);
        expect(decode(qr).text).toBe(text);
      }
    }
  });

  test("finders, timing and the dark module are where the spec puts them", () => {
    const qr = qrEncode(URI);
    const m = qr.modules, n = qr.size;
    for (const [x0, y0] of [[0, 0], [n - 7, 0], [0, n - 7]] as const) {
      const row = (y: number) => Array.from({ length: 7 }, (_, i) => (m[y0 + y]![x0 + i] ? 1 : 0)).join("");
      expect([0, 1, 2, 3, 4, 5, 6].map(row)).toEqual(["1111111", "1000001", "1011101", "1011101", "1011101", "1000001", "1111111"]);
    }
    for (let i = 8; i < n - 8; i++) { expect(m[6]![i]).toBe(i % 2 === 0); expect(m[i]![6]).toBe(i % 2 === 0); }
    expect(m[n - 8]![8]).toBe(true);
  });

  test("the SVG path draws exactly the dark modules", () => {
    const qr = qrEncode("hello");
    const d = qrPath(qr, 4);
    let dark = 0;
    for (const r of qr.modules) for (const c of r) if (c) dark++;
    const widths = [...d.matchAll(/h(\d+)v1/g)].reduce((a, x) => a + Number(x[1]), 0);
    expect(widths).toBe(dark);
    expect(d.startsWith("M4 4h7v1h-7z")).toBe(true);
  });
});
