import { describe, expect, test } from "bun:test";
import { pulse, spawnersOf } from "../src/viewer/spawners";

describe("scripted spawners", () => {
  test("read from the glb's asset extras, defensively", () => {
    const s = spawnersOf({
      spawners: [
        { script: "salvo", effect: "fx\\boom", at: "flag_tower", pos: [1, 2, 3], looping: true, period: 2.5 },
        { script: "once", effect: "fx\\pop", at: "crate", pos: [4, 5, 6], looping: false },
        { pos: [1, 2] },
        { pos: [1, "x", 3] },
        null,
        { script: "neg", pos: [0, 0, 0], looping: true, period: -1 },
      ],
    });
    expect(s).toEqual([
      { script: "salvo", effect: "fx\\boom", at: "flag_tower", pos: [1, 2, 3], looping: true, period: 2.5 },
      { script: "once", effect: "fx\\pop", at: "crate", pos: [4, 5, 6], looping: false, period: null },
      { script: "neg", effect: "", at: "", pos: [0, 0, 0], looping: true, period: null },
    ]);
    expect(spawnersOf(null)).toEqual([]);
    expect(spawnersOf({ spawners: "no" })).toEqual([]);
  });

  test("a looping spawner bursts once a period", () => {
    const s = { looping: true, period: 2 };
    expect(pulse(s, 0)).toBe(1);
    expect(pulse(s, 0.25)).toBeCloseTo(0.5, 9);
    expect(pulse(s, 1)).toBe(0);
    expect(pulse(s, 2)).toBe(1);
    expect(pulse(s, 4.25)).toBeCloseTo(0.5, 9);
    // Faster than the fade: it fades over its own period.
    expect(pulse({ looping: true, period: 0.2 }, 0.1)).toBeCloseTo(0.5, 9);
    // No period, or played once: a steady half.
    expect(pulse({ looping: true, period: null }, 3)).toBe(0.5);
    expect(pulse({ looping: false, period: 2 }, 0)).toBe(0.5);
  });
});
