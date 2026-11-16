import { describe, expect, test } from "bun:test";
import { BODY_MS, Bodies, MAX_BODIES, bodyPlace } from "../src/viewer/bodies";

const at = (x: number, y = 0, z = 0, yaw = 0) => ({ x, y, z, yaw });

describe("dead players' bodies", () => {
  test("a body lies where the frames put the dead player, else where they last stood", () => {
    expect(bodyPlace(at(4, 5, 6, 1), at(1))).toEqual(at(4, 5, 6, 1));
    // An older sender puts the dead at the origin: they lie where they fell.
    expect(bodyPlace(at(0, 0, 0, 2), at(7, 8, 9, 0.5))).toEqual(at(7, 8, 9, 0.5));
    expect(bodyPlace(at(0, 0, 0), null)).toEqual(at(0, 0, 0));
  });

  test("it follows its player while they are dead, then stays behind when they respawn", () => {
    const b = new Bodies<string>();
    b.add(3, at(1), 0, "first");
    expect(b.following(3)?.obj).toBe("first");
    b.release(3);
    expect(b.following(3)).toBeUndefined();
    // Dying again: a new body, the old one stays.
    b.add(3, at(2), 1000, "second");
    expect(b.list.map((x) => [x.obj, x.following])).toEqual([["first", false], ["second", true]]);
  });

  test("bodies go when old, the oldest first past the cap, and all at once on a seek", () => {
    const b = new Bodies<number>();
    for (let i = 0; i < MAX_BODIES; i++) expect(b.add(i, at(i), i, i)).toEqual([]);
    const out = b.add(99, at(99), 100, 99);
    expect(out.map((x) => x.obj)).toEqual([0]);
    expect(b.list).toHaveLength(MAX_BODIES);
    expect(b.expire(BODY_MS + 5).map((x) => x.obj).sort((p, q) => p - q)).toEqual([1, 2, 3, 4, 5]);
    // A clock that went back (a replay started over) clears them too.
    expect(b.expire(-1).length).toBe(MAX_BODIES - 5);
    b.add(1, at(1), 0, 1);
    expect(b.clear()).toHaveLength(1);
    expect(b.list).toHaveLength(0);
  });
});
