// The dense dashboard's data shaping (docs/live/web-boundary.md §3): number
// and byte formats, daily buckets, scales and ticks, stacking, top-N with
// "other", deltas and the crosshair's nearest point. Pure, so the charts
// in ./Deck.tsx stay thin and every number they draw is tested here.

/** One UTC day, "YYYY-MM-DD". */
export type Day = string;

const DAY_MS = 86_400_000;

export function dayOf(ms: number): Day {
  return new Date(ms).toISOString().slice(0, 10);
}

export function dayMs(d: Day): number {
  return Date.parse(`${d}T00:00:00Z`);
}

/** The `n` days ending on `last`, oldest first. */
export function dayRange(last: Day, n: number): Day[] {
  const end = dayMs(last);
  const out: Day[] = [];
  for (let i = n - 1; i >= 0; i--) out.push(dayOf(end - i * DAY_MS));
  return out;
}

/** Daily rows put on a full run of days: a day with no row gets `zero()`,
 *  rows outside the range are dropped. The server only sends days that
 *  had something, so every chart goes through this. */
export function fillDays<T extends { day: Day }>(rows: readonly T[], days: readonly Day[], zero: (day: Day) => T): T[] {
  const by = new Map(rows.map((r) => [r.day, r]));
  return days.map((d) => by.get(d) ?? zero(d));
}

/** 1,284 / 12.9K / 4.2M / 1.1B: full under 10,000, compact above. */
export function compact(n: number): string {
  const a = Math.abs(n);
  if (a < 10_000) return Math.round(n).toLocaleString("en-US");
  const [div, unit] = a < 1e6 ? [1e3, "K"] : a < 1e9 ? [1e6, "M"] : [1e9, "B"];
  const v = n / div;
  return `${Math.abs(v) < 100 ? v.toFixed(1).replace(/\.0$/, "") : Math.round(v)}${unit}`;
}

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** Bytes in powers of 1024, one decimal under 100: 512 B, 3.4 MB, 120 GB. */
export function bytes(n: number): string {
  let v = Math.max(0, n), i = 0;
  while (v >= 1024 && i < BYTE_UNITS.length - 1) { v /= 1024; i++; }
  const s = i === 0 ? String(Math.round(v)) : v < 100 ? v.toFixed(1).replace(/\.0$/, "") : String(Math.round(v));
  return `${s} ${BYTE_UNITS[i]}`;
}

/** Clean axis ticks from 0 to at least `max`, about `count` intervals: steps
 *  of 1, 2 or 5 times a power of ten (for bytes, a power of two of a binary
 *  unit once past 1 KB, so the labels come out whole). */
export function niceTicks(max: number, count = 4, base: 10 | 1024 = 10): number[] {
  if (!(max > 0)) return [0, 1];
  const raw = max / count;
  let steps: number[];
  if (base === 1024 && raw >= 1024) {
    const p = 1024 ** Math.floor(Math.log(raw) / Math.log(1024));
    steps = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024].map((m) => m * p);
  } else {
    const p = 10 ** Math.floor(Math.log10(raw));
    steps = [1, 2, 5, 10].map((m) => m * p);
  }
  // The step whose interval count is nearest `count`; on a tie, the larger.
  const off = (s: number) => Math.abs(Math.ceil(max / s - 1e-9) - count);
  const step = steps.reduce((best, s) => (off(s) <= off(best) ? s : best));
  const ticks: number[] = [];
  for (let i = 0; i * step < max + step * 1e-9; i++) ticks.push(Number((i * step).toPrecision(12)));
  if (ticks[ticks.length - 1]! < max) ticks.push(Number((ticks.length * step).toPrecision(12)));
  return ticks;
}

/** A linear map from [d0, d1] to [r0, r1]. */
export function scale(d0: number, d1: number, r0: number, r1: number): (v: number) => number {
  const k = d1 === d0 ? 0 : (r1 - r0) / (d1 - d0);
  return (v) => r0 + (v - d0) * k;
}

/** An SVG path through the points, or "" for none. */
export function linePath(pts: readonly (readonly [number, number])[]): string {
  return pts.map(([x, y], i) => `${i ? "L" : "M"}${round(x)} ${round(y)}`).join("");
}

/** The area under a line down to `base`. */
export function areaPath(pts: readonly (readonly [number, number])[], base: number): string {
  if (!pts.length) return "";
  return `${linePath(pts)}L${round(pts[pts.length - 1]![0])} ${round(base)}L${round(pts[0]![0])} ${round(base)}Z`;
}

const round = (v: number) => Math.round(v * 10) / 10;

/** A column with a 4px rounded data end and a square base (marks-and-anatomy:
 *  bars grow from one baseline). `y` is the top, `h` the height. */
export function columnPath(x: number, y: number, w: number, h: number, r = 4): string {
  if (h <= 0 || w <= 0) return "";
  const rr = Math.min(r, w / 2, h);
  return `M${round(x)} ${round(y + h)}V${round(y + rr)}Q${round(x)} ${round(y)} ${round(x + rr)} ${round(y)}H${round(x + w - rr)}Q${round(x + w)} ${round(y)} ${round(x + w)} ${round(y + rr)}V${round(y + h)}Z`;
}

export interface Series { key: string; label: string; values: readonly number[] }

/** A row of ranked bars; `other` is the folded tail. */
export interface RankRow { key: string; label: string; value: number; other?: boolean; href?: string }

/** Stacked segments per index, bottom first: `[y0, y1]` per series, in the series' order. */
export function stack(series: readonly Series[]): { key: string; spans: [number, number][] }[] {
  const n = Math.max(0, ...series.map((s) => s.values.length));
  const floor = new Array<number>(n).fill(0);
  return series.map((s) => ({
    key: s.key,
    spans: Array.from({ length: n }, (_, i) => {
      const y0 = floor[i]!, y1 = y0 + Math.max(0, s.values[i] ?? 0);
      floor[i] = y1;
      return [y0, y1] as [number, number];
    }),
  }));
}

/** The tallest stack, or the highest point of any series. */
export function peak(series: readonly Series[], stacked: boolean): number {
  const n = Math.max(0, ...series.map((s) => s.values.length));
  let m = 0;
  for (let i = 0; i < n; i++) {
    const vs = series.map((s) => Math.max(0, s.values[i] ?? 0));
    m = Math.max(m, stacked ? vs.reduce((a, b) => a + b, 0) : Math.max(0, ...vs));
  }
  return m;
}

/** The `n` largest by `value`, the rest folded into one "other" row
 *  (made by `other(sum, count)`) when there is a rest. Ties keep input order. */
export function topN<T>(rows: readonly T[], n: number, value: (r: T) => number, other?: (sum: number, count: number) => T): T[] {
  const sorted = rows.map((r, i) => [r, i] as const).sort((a, b) => value(b[0]) - value(a[0]) || a[1] - b[1]).map(([r]) => r);
  if (sorted.length <= n || !other) return sorted.slice(0, n);
  const rest = sorted.slice(n);
  return [...sorted.slice(0, n), other(rest.reduce((a, r) => a + value(r), 0), rest.length)];
}

export interface Delta { pct: number | null; dir: "up" | "down" | "flat"; text: string }

/** This period against the one before: +12%, −4%, "new" when there was nothing before. */
export function delta(now: number, before: number): Delta {
  if (before === 0) return now === 0 ? { pct: 0, dir: "flat", text: "0%" } : { pct: null, dir: "up", text: "new" };
  const pct = ((now - before) / before) * 100;
  const r = Math.round(pct);
  const dir = r > 0 ? "up" : r < 0 ? "down" : "flat";
  return { pct, dir, text: `${r > 0 ? "+" : r < 0 ? "−" : ""}${Math.abs(r)}%` };
}

/** Sums of the last `n` and the `n` before them. */
export function periods(values: readonly number[], n: number): { now: number; before: number } {
  const sum = (a: readonly number[]) => a.reduce((x, y) => x + y, 0);
  const len = values.length;
  return { now: sum(values.slice(Math.max(0, len - n))), before: sum(values.slice(Math.max(0, len - 2 * n), Math.max(0, len - n))) };
}

/** The index of the slot nearest `x` among `count` evenly spaced from `x0` to `x1` (the crosshair's snap). */
export function nearestIndex(x: number, x0: number, x1: number, count: number): number {
  if (count <= 1 || x1 === x0) return 0;
  const i = Math.round(((x - x0) / (x1 - x0)) * (count - 1));
  return Math.min(count - 1, Math.max(0, i));
}

/** Each part's share of the whole, in 0..1; all zero when the whole is. */
export function shares(values: readonly number[]): number[] {
  const t = values.reduce((a, b) => a + Math.max(0, b), 0);
  return values.map((v) => (t > 0 ? Math.max(0, v) / t : 0));
}

export type SortDir = "asc" | "desc";

/** Rows sorted by a column, numbers before text, stable. */
export function sortRows<T>(rows: readonly T[], key: (r: T) => number | string, dir: SortDir): T[] {
  const m = dir === "asc" ? 1 : -1;
  return rows.map((r, i) => [r, i] as const).sort((a, b) => {
    const x = key(a[0]), y = key(b[0]);
    const c = typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y));
    return c * m || a[1] - b[1];
  }).map(([r]) => r);
}

/** "Nov 18" for an axis or tooltip. */
export function shortDay(d: Day): string {
  return new Date(dayMs(d)).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}
