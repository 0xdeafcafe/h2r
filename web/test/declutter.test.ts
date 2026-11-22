import { describe, expect, test } from "bun:test";
import { declutter, type TagIn } from "../src/viewer/declutter";

function tag(id: number, fields: Partial<TagIn> = {}): TagIn {
  return { id, x: 100, y: 100, w: 60, h: 20, depth: 10, rank: 3, ...fields };
}

describe("viewer tag declutter", () => {
  test("overlapping tags are placed full, then lifted, then pip", () => {
    const result = declutter([tag(0), tag(1), tag(2)], new Map(), 0);
    expect([...result.values()].map(({ lod }) => lod)).toEqual(["full", "lifted", "pip"]);
  });

  test("rank beats distance and distance breaks rank ties", () => {
    const tags = [tag(0, { depth: 3 }), tag(1, { rank: 0, depth: 50 }), tag(2, { depth: 1 })];
    const result = declutter(tags, new Map(), 0);
    expect(result.get(1)?.lod).toBe("full");
    expect(result.get(2)?.lod).toBe("lifted");
    expect(result.get(0)?.lod).toBe("pip");
  });

  test("the same candidate must hold for 250 ms before the tag changes level", () => {
    const apart = [tag(0), tag(1, { x: 180 })];
    const crowded = [tag(0), tag(1)];
    let state = declutter(apart, new Map(), 0);
    state = declutter(crowded, state, 10);
    expect(declutter(crowded, state, 259).get(1)?.lod).toBe("full");
    expect(declutter(crowded, state, 260).get(1)?.lod).toBe("lifted");
    state = declutter(apart, state, 100);
    state = declutter(crowded, state, 200);
    expect(declutter(crowded, state, 449).get(1)?.lod).toBe("full");
    expect(declutter(crowded, state, 450).get(1)?.lod).toBe("lifted");
  });

  test("an existing full tag keeps its level just inside the overlap threshold", () => {
    let state = declutter([tag(0), tag(1, { x: 180 })], new Map(), 0);
    const close = [tag(0), tag(1, { x: 157 })];
    state = declutter(close, state, 1);
    expect(declutter(close, state, 251).get(1)?.lod).toBe("full");
    const overlap = [tag(0), tag(1, { x: 156 })];
    state = declutter(overlap, state, 252);
    expect(declutter(overlap, state, 502).get(1)?.lod).toBe("lifted");
  });

  test("full and lifted rectangles do not intersect in 500 seeded layouts", () => {
    let seed = 27;
    const random = () => {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      return seed / 2 ** 32;
    };
    for (let layout = 0; layout < 500; layout++) {
      const tags = Array.from({ length: 64 }, (_, id) => tag(id, {
        x: random() * 400,
        y: random() * 220,
        w: 30 + random() * 70,
        h: 12 + random() * 15,
        depth: random() * 30,
        rank: Math.floor(random() * 5),
      }));
      const result = declutter(tags, new Map(), 0);
      const rectangles = tags.flatMap(({ id, x, y, w, h }) => {
        const lod = result.get(id)?.lod;
        if (lod === "pip") return [];
        const bottom = lod === "lifted" ? y - h - 3 : y;
        return [{ left: x - w / 2, right: x + w / 2, top: bottom - h, bottom }];
      });
      const hasIntersection = rectangles.some((a, i) => rectangles.slice(0, i).some((b) =>
        a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom,
      ));
      expect(hasIntersection).toBe(false);
    }
  });

  test("at most 64 tags are laid out, with priority deciding the limit", () => {
    const tags = Array.from({ length: 70 }, (_, id) => tag(id, { x: id * 100, rank: id === 69 ? 0 : 4 }));
    const result = declutter(tags, new Map(), 0);
    expect([...result.values()].filter(({ lod }) => lod === "full")).toHaveLength(64);
    expect(result.get(69)?.lod).toBe("full");
    expect(result.get(63)?.lod).toBe("pip");
  });
});
