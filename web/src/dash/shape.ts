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

// ---- the main menu's ring ----
// The menu items ride a ring round the orb. The selected one sits in the
// focus slot; the ones after it trail down the orb's right side, the ones
// before it climb over the top, shrinking and dimming as they go.

export interface RingSlot {
  /** Degrees round the orb, 0 pointing right, positive downwards. */
  angle: number;
  /** Distance of the pod's centre from the orb's, in stage units. */
  radius: number;
  scale: number;
  opacity: number;
  /** How far the bar's centre hangs below its pod's, in stage units (before scaling): more the further down the ring. */
  drop: number;
}

/** Slots by whole steps from the focus: index 5 is the focus, 0 is five before it, 10 five after. */
const RING: RingSlot[] = [
  { angle: -116, radius: 276, scale: 0.34, opacity: 0, drop: -10 },
  { angle: -102, radius: 264, scale: 0.44, opacity: 0.3, drop: -8 },
  { angle: -86, radius: 240, scale: 0.56, opacity: 0.45, drop: 3 },
  { angle: -68, radius: 230, scale: 0.7, opacity: 0.62, drop: 5 },
  { angle: -46.5, radius: 221, scale: 0.86, opacity: 0.82, drop: 9 },
  { angle: -22.6, radius: 214, scale: 1, opacity: 1, drop: 13 },
  { angle: 2.5, radius: 213, scale: 1, opacity: 1, drop: 20 },
  { angle: 25.4, radius: 215, scale: 1, opacity: 1, drop: 26 },
  { angle: 48.8, radius: 217, scale: 0.98, opacity: 0.96, drop: 29 },
  { angle: 70.3, radius: 221, scale: 0.95, opacity: 0.9, drop: 35 },
  { angle: 90, radius: 228, scale: 0.86, opacity: 0, drop: 40 },
];
const RING_FOCUS = 5;

const lerp = (a: number, b: number, t: number) => a + (b - a) * t;

/** The slot `d` steps from the focus (fractions blend neighbours; past the ends it stays hidden). */
export function ringSlot(d: number): RingSlot {
  const p = Math.min(RING.length - 1, Math.max(0, d + RING_FOCUS));
  const i = Math.min(RING.length - 2, Math.floor(p));
  const t = p - i;
  const a = RING[i]!, b = RING[i + 1]!;
  return { angle: lerp(a.angle, b.angle, t), radius: lerp(a.radius, b.radius, t), scale: lerp(a.scale, b.scale, t), opacity: lerp(a.opacity, b.opacity, t), drop: lerp(a.drop, b.drop, t) };
}

export interface RingPlace { x: number; y: number; scale: number; opacity: number; drop: number; lit: number }

/** Where an item `d` steps from the focus sits round an orb at (cx, cy), and how lit it is (1 in focus, 0 a step away). */
export function ringPlace(d: number, cx: number, cy: number): RingPlace {
  const s = ringSlot(d);
  const a = (s.angle * Math.PI) / 180;
  const lit = Math.min(1, Math.max(0, (0.85 - Math.abs(d)) / 0.5));
  return { x: cx + Math.cos(a) * s.radius, y: cy + Math.sin(a) * s.radius, scale: s.scale, opacity: s.opacity, drop: s.drop, lit };
}

/** One frame of easing `from` towards `to`: closes all but e^(-dt/tau) of the gap, and lands when close. */
export function approach(from: number, to: number, dtMs: number, tauMs: number): number {
  const next = to + (from - to) * Math.exp(-Math.max(0, dtMs) / tauMs);
  return Math.abs(to - next) < 0.002 ? to : next;
}
