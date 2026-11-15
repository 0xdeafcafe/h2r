// Shapes the dashboard draws for itself: a repeatable hash for the
// tunnel's mesh and the orb's jelly outlines.

/** A repeatable value in [0, 1) for a pair of integers. */
export function hash01(a: number, b: number): number {
  let h = (Math.imul(a | 0, 374761393) + Math.imul(b | 0, 668265263)) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

export type Point = [number, number];

/** `n` points round the origin, each between r * (1 - amp) and r * (1 + amp) away. */
export function blobPoints(seed: number, n: number, r: number, amp: number): Point[] {
  const pts: Point[] = [];
  for (let k = 0; k < n; k++) {
    const a = ((k + (hash01(seed, k + 97) - 0.5) * 0.5) / n) * Math.PI * 2;
    const rr = r * (1 + (hash01(seed, k) * 2 - 1) * amp);
    pts.push([Math.cos(a) * rr, Math.sin(a) * rr]);
  }
  return pts;
}

const f = (v: number) => (Math.round(v * 100) / 100).toString();

/** A smooth closed SVG path through the points (Catmull-Rom as cubic Beziers). */
export function closedPath(pts: Point[]): string {
  const n = pts.length;
  if (n < 3) return "";
  const at = (i: number) => pts[((i % n) + n) % n]!;
  let d = `M${f(pts[0]![0])} ${f(pts[0]![1])}`;
  for (let i = 0; i < n; i++) {
    const p0 = at(i - 1), p1 = at(i), p2 = at(i + 1), p3 = at(i + 2);
    d += `C${f(p1[0] + (p2[0] - p0[0]) / 6)} ${f(p1[1] + (p2[1] - p0[1]) / 6)} ${f(p2[0] - (p3[0] - p1[0]) / 6)} ${f(p2[1] - (p3[1] - p1[1]) / 6)} ${f(p2[0])} ${f(p2[1])}`;
  }
  return d + "Z";
}

/** A wobbly closed outline, the same for the same seed. */
export const blobPath = (seed: number, n: number, r: number, amp: number) => closedPath(blobPoints(seed, n, r, amp));
