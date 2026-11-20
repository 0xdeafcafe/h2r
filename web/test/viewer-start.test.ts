import { describe, expect, test } from "bun:test";
import { fitBox, framingPitch, type Box3 } from "../src/viewer/camera";
import { addressPlayer, easeOutExpo, playableBounds, startCamera } from "../src/viewer/start";
import type { Vec3 } from "../src/map/views";

const roster = [
  { index: 4, name: "Point", game_name: "In Game", xuid: "AB12" },
  { index: 2, name: "Point", game_name: null, xuid: "CD34" },
  { index: 7, name: "Self", game_name: null, xuid: "ME" },
  { index: 1, name: "Focus", game_name: null, xuid: null },
];
const here = (index: number) => ({ index, alive: false });
const player = (index: number) => ({ kind: "player" as const, index });

test("the address accepts names and xuids after matching case and whitespace", () => {
  expect(addressPlayer(roster, " pOiNt ")).toBe(2);
  expect(addressPlayer(roster, "in GAME")).toBe(4);
  expect(addressPlayer(roster, "ab12")).toBe(4);
});

test("the address, signed-in player, focus and playable bounds each take their turn", () => {
  const present = roster.map(p => here(p.index));
  expect(startCamera(roster, present, "Point", "ME", 1)).toEqual(player(2));
  expect(startCamera(roster, [here(4), here(7), here(1)], "Point", "ME", 1)).toEqual(player(7));
  expect(startCamera(roster, present, "Unknown", "ME", 1)).toEqual(player(7));
  expect(startCamera(roster, [here(1)], "Point", "ME", 1)).toEqual(player(1));
  expect(startCamera(roster, [], "Point", "ME", 1)).toEqual({ kind: "bounds" });
});

describe("playable bounds", () => {
  const level: Box3 = { min: [-40, -40, -4], max: [40, 40, 16] };
  const spots = (size: number) => Array.from({ length: 6 }, (_, i) => ({
    kind: "weap", at: [i % 2 ? size : -size, i % 3 ? size : -size, i % 2 ? 2 : 0] as Vec3,
  }));

  test("pads the placements, includes features, and widens narrow axes", () => {
    expect(playableBounds(level, spots(10), null)).toEqual({ min: [-14, -14, -1], max: [14, 14, 5] });
    const fit = playableBounds(level, spots(10).slice(0, 5), null, [{ x: 18, y: 0, z: 0 }]);
    expect(fit.max[0]).toBe(22);
    expect(playableBounds(level, [], { minX: 0, maxX: 0, minY: 0, maxY: 0 }).max[2]).toBe(0.5);
  });

  test("sparse placements and a fit that covers over 85% use the full box", () => {
    expect(playableBounds(level, spots(10).slice(0, 5), null)).toBe(level);
    expect(playableBounds(level, spots(32), null)).toBe(level);
  });
});

test("the eight corners project inside the margin in wide and tall views", () => {
  const box: Box3 = { min: [-16, -7, -2], max: [16, 7, 2] };
  const dir: Vec3 = [0.6, 0, 0.8];
  const tan = Math.tan(42 * Math.PI / 360);
  for (const aspect of [16 / 9, 4 / 3, 9 / 16]) {
    const d = fitBox(box, dir, 42, aspect, 0.10);
    for (const x of [-16, 16]) for (const y of [-7, 7]) for (const z of [-2, 2]) {
      const depth = d - (0.6 * x + 0.8 * z);
      expect(Math.abs(y) / (depth * tan * aspect)).toBeLessThanOrEqual(0.9000001);
      expect(Math.abs(-0.8 * x + 0.6 * z) / (depth * tan)).toBeLessThanOrEqual(0.9000001);
    }
  }
});

test("pitch and opening ease reach their limits", () => {
  expect(framingPitch([100, 20, 0])).toBe(44);
  expect(framingPitch([40, 20, 10])).toBe(35);
  expect(framingPitch([40, 10, 40])).toBe(26);
  expect(easeOutExpo(0)).toBe(0);
  expect(easeOutExpo(0.5)).toBeGreaterThan(0.9);
  expect(easeOutExpo(1)).toBe(1);
});
