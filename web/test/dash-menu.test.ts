import { describe, expect, test } from "bun:test";
import { approach, ringPlace, ringSlot } from "../src/dash/shape";
import { crosses, sideOf } from "../src/dash/cross";

describe("the menu ring", () => {
  test("the focus slot is full size, fully lit, right of the orb and a little above its centre", () => {
    const p = ringPlace(0, 444, 318);
    expect(p.scale).toBe(1);
    expect(p.opacity).toBe(1);
    expect(p.lit).toBe(1);
    expect(p.x).toBeGreaterThan(444 + 180);
    expect(p.y).toBeLessThan(318);
  });

  test("items after the focus run down the orb's side, items before it climb over the top", () => {
    let prev = ringPlace(-4, 0, 0).y;
    for (let d = -3; d <= 4; d++) {
      const y = ringPlace(d, 0, 0).y;
      expect(y).toBeGreaterThan(prev);
      prev = y;
    }
  });

  test("bars a step apart never overlap: their centres are further apart than the scaled bar height", () => {
    const BAR = 35;
    const barY = (d: number) => { const p = ringPlace(d, 0, 0); return p.y + p.drop * p.scale; };
    for (let d = -4; d < 4; d++) {
      const a = ringPlace(d, 0, 0), b = ringPlace(d + 1, 0, 0);
      expect(barY(d + 1) - barY(d)).toBeGreaterThanOrEqual((BAR * (a.scale + b.scale)) / 2);
    }
  });

  test("bars hang lower off their pods the further down the ring they are", () => {
    for (let d = -4; d < 4; d++) expect(ringSlot(d + 1).drop).toBeGreaterThan(ringSlot(d).drop);
    expect(ringSlot(0.5).drop).toBeCloseTo((ringSlot(0).drop + ringSlot(1).drop) / 2, 6);
  });

  test("five items are all visible whichever is in focus; further round they are gone", () => {
    for (let d = -4; d <= 4; d++) expect(ringSlot(d).opacity).toBeGreaterThan(0.25);
    expect(ringSlot(5).opacity).toBe(0);
    expect(ringSlot(-5).opacity).toBe(0);
    expect(ringSlot(40).opacity).toBe(0);
    expect(ringSlot(-40).opacity).toBe(0);
  });

  test("fractions blend neighbouring slots, and only the item near the focus is lit", () => {
    const a = ringSlot(1), b = ringSlot(2), mid = ringSlot(1.5);
    expect(mid.angle).toBeCloseTo((a.angle + b.angle) / 2, 6);
    expect(mid.radius).toBeCloseTo((a.radius + b.radius) / 2, 6);
    expect(ringPlace(0.3, 0, 0).lit).toBe(1);
    expect(ringPlace(0.6, 0, 0).lit).toBeCloseTo(0.5, 6);
    expect(ringPlace(1, 0, 0).lit).toBe(0);
    expect(ringPlace(-1, 0, 0).lit).toBe(0);
  });
});

describe("approach", () => {
  test("closes the gap without overshooting and lands exactly", () => {
    let v = 0;
    for (let i = 0; i < 200 && v !== 3; i++) {
      const next = approach(v, 3, 16, 95);
      expect(next).toBeGreaterThan(v);
      expect(next).toBeLessThanOrEqual(3);
      v = next;
    }
    expect(v).toBe(3);
  });

  test("is settled within about half a second", () => {
    let v = 4, t = 0;
    while (v !== 0 && t < 5000) { v = approach(v, 0, 16, 95); t += 16; }
    expect(t).toBeLessThan(800);
    expect(approach(1, 1, 16, 95)).toBe(1);
    expect(approach(0, 1, 0, 95)).toBe(0);
  });
});

describe("sides of the site", () => {
  test("the dashboard's paths", () => {
    for (const p of ["/", "", "/dash", "/dash/players", "/dash/players/Some%20One", "/dash/settings/", "/players", "/?server=https://x.example"]) expect(sideOf(p)).toBe("dash");
  });
  test("everything else is the stats site", () => {
    for (const p of ["/stats", "/stats/games/abc", "/stats/players/x/career?compare=y", "/games/1", "/players/x", "/events", "/dashboard"]) expect(sideOf(p)).toBe("stats");
  });
  test("crossing is a change of side", () => {
    expect(crosses("/", "/stats")).toBe(true);
    expect(crosses("/stats/events", "/dash/server")).toBe(true);
    expect(crosses("/dash/players", "/")).toBe(false);
    expect(crosses("/stats", "/stats/players")).toBe(false);
  });
});
