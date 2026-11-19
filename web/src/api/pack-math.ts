import { PackError } from "./pack";

const TAU = Math.PI * 2;
const DIR_MAX = 16382;

/** Rust/JS use the same binary64 operations for the fitted curve. */
export function sin(x: number): number {
  let a = x - TAU * Math.floor(x / TAU + 0.5);
  if (a > Math.PI / 2) a = Math.PI - a;
  if (a < -Math.PI / 2) a = -Math.PI - a;
  const z = -a * a;
  let p = 1;
  for (let k = 9; k >= 1; k--) p = 1 + z * p / ((2 * k) * (2 * k + 1));
  return a * p;
}

export function oct(x: number, y: number, z: number): [number, number] {
  const l = Math.abs(x) + Math.abs(y) + Math.abs(z);
  if (!Number.isFinite(l) || l < 1e-12) throw new PackError("invalid octahedral direction");
  x /= l; y /= l;
  if (z < 0) {
    const t = (1 - Math.abs(y)) * (x >= 0 ? 1 : -1);
    y = (1 - Math.abs(x)) * (y >= 0 ? 1 : -1);
    x = t;
  }
  const q = (v: number) => Math.round((Math.min(1, Math.max(-1, v)) + 1) * 0.5 * DIR_MAX);
  return [q(x), q(y)];
}

export function rotatedOct(v: readonly number[], delta: number): [number, number] {
  const c = sin(delta + Math.PI / 2), s = sin(delta);
  return oct(v[0]! * c - v[1]! * s, v[0]! * s + v[1]! * c, v[2]!);
}
