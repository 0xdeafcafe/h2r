import { describe, expect, test } from "bun:test";
import { skullSummary } from "../src/stats/skullSummary";

describe("skullSummary", () => {
  test("empty for no filter", () => expect(skullSummary([])).toBe(""));
  test("names one or two skulls", () => {
    expect(skullSummary([2])).toBe("Skulls: Assassins");
    expect(skullSummary([2, 3])).toBe("Skulls: Assassins, Thunderstorm");
  });
  test("counts three or more", () => expect(skullSummary([2, 3, 4])).toBe("3 skulls"));
});
