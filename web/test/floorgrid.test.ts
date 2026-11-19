import { describe, expect, test } from "bun:test";
import { FloorGrid } from "../src/viewer/floorgrid";

// A floor quad at height y over [x0, x1] x [z0, z1], wound to face up (y+).
function quad(x0: number, x1: number, z0: number, z1: number, y: number): number[] {
  // (x0,z1) (x1,z1) (x1,z0) / (x0,z1) (x1,z0) (x0,z0): counter-clockwise seen from above.
  return [x0, y, z1, x1, y, z1, x1, y, z0, x0, y, z1, x1, y, z0, x0, y, z0];
}

describe("FloorGrid", () => {
  test("finds the highest floor below within reach", () => {
    const g = new FloorGrid([
      { id: 0, positions: quad(-10, 10, -10, 10, 0) },
      { id: 1, positions: quad(-1, 1, -1, 1, 2) },
    ]);
    expect(g.probe(0, 5, 0, 10)).toMatchObject({ y: 2, id: 1 });
    expect(g.probe(0, 1.5, 0, 10)).toMatchObject({ y: 0, id: 0 });
    expect(g.probe(5, 5, 5, 10)).toMatchObject({ y: 0, id: 0 });
    // Out of reach, and outside the level.
    expect(g.probe(5, 5, 5, 4)).toBeNull();
    expect(g.probe(50, 5, 50, 100)).toBeNull();
    const n = g.probe(0, 5, 0, 10)!;
    expect([n.nx, n.ny, n.nz].map((v) => Math.round(v * 1e6) / 1e6 + 0)).toEqual([0, 1, 0]);
  });

  test("skips sources that are not shown, and faces turned down", () => {
    const down = quad(-1, 1, -1, 1, 3);
    // Same corners, wound the other way: a ceiling seen from above.
    const ceiling = [down[0], down[1], down[2], down[6], down[7], down[8], down[3], down[4], down[5]] as number[];
    const g = new FloorGrid([
      { id: 0, positions: quad(-10, 10, -10, 10, 0) },
      { id: 1, positions: quad(-1, 1, -1, 1, 2) },
      { id: 2, positions: ceiling },
    ]);
    expect(g.probe(0.5, 5, 0.9, 10)!.id).toBe(1);
    expect(g.probe(0, 5, 0, 10, (id) => id !== 1)).toMatchObject({ y: 0, id: 0 });
  });

  test("uses indices, slopes, and wide triangles", () => {
    // A ramp rising along x: y = x / 2 over [0, 4] x [0, 4], indexed.
    const positions = [0, 0, 4, 4, 2, 4, 4, 2, 0, 0, 0, 0];
    const g = new FloorGrid([
      { id: 0, positions, index: [0, 1, 2, 0, 2, 3] },
      // Many small floor tiles far away, so the far plane below is "wide".
      ...Array.from({ length: 200 }, (_, i) => ({ id: 1, positions: quad(100 + i, 100.5 + i, 100, 100.5, 0) })),
      { id: 2, positions: quad(-1000, 1000, -1000, 1000, -50) },
    ]);
    expect(g.probe(2, 10, 2, 20)!.y).toBeCloseTo(1, 5);
    const n = g.probe(2, 10, 2, 20)!;
    expect(n.nx).toBeLessThan(0);
    expect(n.ny).toBeGreaterThan(0);
    expect(g.probe(-500, 0, 500, 100)).toMatchObject({ y: -50, id: 2 });
    expect(g.probe(100.25, 1, 100.25, 2)).toMatchObject({ y: 0, id: 1 });
  });

  test("an empty level finds nothing", () => {
    expect(new FloorGrid([]).probe(0, 0, 0, 10)).toBeNull();
  });
});
