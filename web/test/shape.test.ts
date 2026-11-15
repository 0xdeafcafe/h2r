import { describe, expect, test } from "bun:test";
import { blobPath, blobPoints, closedPath, hash01 } from "../src/dash/shape";

describe("hash01", () => {
  test("is repeatable and stays in [0, 1)", () => {
    for (let a = -40; a < 40; a++) {
      for (let b = 0; b < 40; b++) {
        const v = hash01(a, b);
        expect(v).toBe(hash01(a, b));
        expect(v).toBeGreaterThanOrEqual(0);
        expect(v).toBeLessThan(1);
      }
    }
  });

  test("spreads its values", () => {
    let low = 0;
    const n = 4000;
    for (let i = 0; i < n; i++) if (hash01(i - 2000, i * 7) < 0.5) low++;
    expect(low / n).toBeGreaterThan(0.44);
    expect(low / n).toBeLessThan(0.56);
    expect(hash01(1, 2)).not.toBe(hash01(2, 1));
  });
});

describe("blobs", () => {
  test("points stay within the wobble and go round once", () => {
    const pts = blobPoints(7, 9, 100, 0.1);
    expect(pts).toHaveLength(9);
    let last = -Infinity;
    for (const [x, y] of pts) {
      const r = Math.hypot(x, y);
      expect(r).toBeGreaterThanOrEqual(90 - 1e-9);
      expect(r).toBeLessThanOrEqual(110 + 1e-9);
      const a = (Math.atan2(y, x) + Math.PI * 2 + 0.3) % (Math.PI * 2);
      expect(a).toBeGreaterThan(last);
      last = a;
    }
  });

  test("the path is closed, has a curve per point and repeats for a seed", () => {
    const d = blobPath(3, 8, 100, 0.08);
    expect(d.startsWith("M")).toBe(true);
    expect(d.endsWith("Z")).toBe(true);
    expect(d.match(/C/g)).toHaveLength(8);
    expect(d).toBe(blobPath(3, 8, 100, 0.08));
    expect(d).not.toBe(blobPath(4, 8, 100, 0.08));
    expect(d).not.toContain("NaN");
  });

  test("a curve ends where the next starts, and the last ends at the first point", () => {
    const pts = blobPoints(11, 6, 50, 0.2);
    const d = closedPath(pts);
    const ends = d.slice(0, -1).split("C").slice(1).map((s) => s.trim().split(/\s+/).slice(-2).map(Number));
    ends.forEach((e, i) => {
      const p = pts[(i + 1) % pts.length]!;
      expect(e[0]).toBeCloseTo(p[0], 1);
      expect(e[1]).toBeCloseTo(p[1], 1);
    });
  });

  test("too few points make no path", () => {
    expect(closedPath([[0, 0], [1, 1]])).toBe("");
  });
});
