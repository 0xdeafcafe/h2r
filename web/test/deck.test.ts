// The dense dashboard's data shaping (src/deck/chart.ts, usageView.ts), its
// fixed sample data and its palette rules.
import { describe, expect, test } from "bun:test";
import {
  areaPath, bytes, columnPath, compact, dayRange, delta, fillDays, linePath, nearestIndex, niceTicks, peak, periods,
  scale, shares, shortDay, sortRows, stack, topN,
} from "../src/deck/chart";
import { SERIES, seriesColor } from "../src/deck/palette";
import { UPLOAD_KINDS, messagesOf, uploaded, usageMock } from "../src/deck/mock";
import { tileValue, usageView } from "../src/deck/usageView";

describe("days", () => {
  test("a range ends on its last day, oldest first, across a month end", () => {
    expect(dayRange("2026-11-02", 4)).toEqual(["2026-10-30", "2026-10-31", "2026-11-01", "2026-11-02"]);
  });

  test("missing days are filled with zeros and days outside the range dropped", () => {
    const rows = [{ day: "2026-11-01", n: 3 }, { day: "2026-10-01", n: 9 }, { day: "2026-11-03", n: 5 }];
    expect(fillDays(rows, dayRange("2026-11-03", 3), (day) => ({ day, n: 0 })).map((r) => r.n)).toEqual([3, 0, 5]);
  });

  test("short day labels are UTC", () => {
    expect(shortDay("2026-11-18")).toBe("Nov 18");
  });
});

describe("number formats", () => {
  test("counts are in full under 10,000 and compact above", () => {
    expect(compact(0)).toBe("0");
    expect(compact(1284)).toBe("1,284");
    expect(compact(9999)).toBe("9,999");
    expect(compact(12_900)).toBe("12.9K");
    expect(compact(250_000)).toBe("250K");
    expect(compact(4_200_000)).toBe("4.2M");
    expect(compact(2_000_000_000)).toBe("2B");
  });

  test("bytes step by 1024 with one decimal under 100", () => {
    expect(bytes(0)).toBe("0 B");
    expect(bytes(512)).toBe("512 B");
    expect(bytes(1536)).toBe("1.5 KB");
    expect(bytes(3.4 * 1024 * 1024)).toBe("3.4 MB");
    expect(bytes(120 * 1024 ** 3)).toBe("120 GB");
    expect(bytes(-5)).toBe("0 B");
  });
});

describe("axes", () => {
  test("ticks are 1, 2 or 5 times a power of ten, from 0 to at least the peak", () => {
    expect(niceTicks(87)).toEqual([0, 20, 40, 60, 80, 100]);
    expect(niceTicks(1000)).toEqual([0, 200, 400, 600, 800, 1000]);
    expect(niceTicks(0.7)).toEqual([0, 0.2, 0.4, 0.6, 0.8]);
    expect(niceTicks(12_345)).toEqual([0, 5000, 10_000, 15_000]);
  });

  test("an empty or zero series still gets an axis", () => {
    expect(niceTicks(0)).toEqual([0, 1]);
    expect(niceTicks(Number.NaN)).toEqual([0, 1]);
  });

  test("byte ticks land on whole binary units", () => {
    const MB = 1024 * 1024;
    const t = niceTicks(300 * MB, 4, 1024);
    expect(t.map((v) => bytes(v))).toEqual(["0 B", "128 MB", "256 MB", "384 MB"]);
  });

  test("a scale maps its domain onto its range, and a flat domain onto its start", () => {
    const s = scale(0, 100, 200, 0);
    expect(s(0)).toBe(200);
    expect(s(50)).toBe(100);
    expect(scale(5, 5, 10, 20)(7)).toBe(10);
  });
});

describe("marks", () => {
  test("lines and areas", () => {
    expect(linePath([])).toBe("");
    expect(linePath([[0, 10], [5.25, 2]])).toBe("M0 10L5.3 2");
    expect(areaPath([[0, 10], [10, 2]], 20)).toBe("M0 10L10 2L10 20L0 20Z");
  });

  test("a column has a rounded data end and a square base, and nothing at zero height", () => {
    const d = columnPath(10, 20, 12, 30);
    expect(d.startsWith("M10 50V24Q10 20 14 20")).toBe(true);
    expect(d.endsWith("V50Z")).toBe(true);
    expect(columnPath(0, 0, 10, 0)).toBe("");
    // A short column's rounding never passes its height.
    expect(columnPath(0, 0, 10, 2)).toContain("V2Q");
  });

  test("stacks run bottom first in series order; negatives count as zero", () => {
    const s = stack([{ key: "a", label: "a", values: [1, 2] }, { key: "b", label: "b", values: [3, -1] }]);
    expect(s.map((x) => x.spans)).toEqual([[[0, 1], [0, 2]], [[1, 4], [2, 2]]]);
    expect(peak([{ key: "a", label: "a", values: [1, 2] }, { key: "b", label: "b", values: [3, -1] }], true)).toBe(4);
    expect(peak([{ key: "a", label: "a", values: [1, 2] }, { key: "b", label: "b", values: [3, -1] }], false)).toBe(3);
  });

  test("shares of a whole, and of nothing", () => {
    expect(shares([1, 3])).toEqual([0.25, 0.75]);
    expect(shares([0, 0])).toEqual([0, 0]);
  });

  test("the crosshair snaps to the nearest slot and stays on the plot", () => {
    expect(nearestIndex(0, 0, 100, 11)).toBe(0);
    expect(nearestIndex(44, 0, 100, 11)).toBe(4);
    expect(nearestIndex(46, 0, 100, 11)).toBe(5);
    expect(nearestIndex(-30, 0, 100, 11)).toBe(0);
    expect(nearestIndex(500, 0, 100, 11)).toBe(10);
    expect(nearestIndex(50, 0, 100, 1)).toBe(0);
  });
});

describe("lists", () => {
  const rows = [{ k: "a", v: 5 }, { k: "b", v: 9 }, { k: "c", v: 1 }, { k: "d", v: 5 }, { k: "e", v: 2 }];

  test("top N folds the rest into one other row; ties keep input order", () => {
    const t = topN(rows, 3, (r) => r.v, (sum, count) => ({ k: `other ${count}`, v: sum }));
    expect(t).toEqual([{ k: "b", v: 9 }, { k: "a", v: 5 }, { k: "d", v: 5 }, { k: "other 2", v: 3 }]);
  });

  test("no other row when everything fits, or when none is asked for", () => {
    expect(topN(rows, 5, (r) => r.v, (s) => ({ k: "x", v: s }))).toHaveLength(5);
    expect(topN(rows, 2, (r) => r.v)).toEqual([{ k: "b", v: 9 }, { k: "a", v: 5 }]);
  });

  test("sorting is stable, numbers numerically, text by locale", () => {
    expect(sortRows(rows, (r) => r.v, "desc").map((r) => r.k)).toEqual(["b", "a", "d", "e", "c"]);
    expect(sortRows(rows, (r) => r.v, "asc").map((r) => r.k)).toEqual(["c", "e", "a", "d", "b"]);
    expect(sortRows([{ n: "beta" }, { n: "Alpha" }], (r) => r.n, "asc").map((r) => r.n)).toEqual(["Alpha", "beta"]);
  });
});

describe("change against the period before", () => {
  test("rounded, signed, with a direction", () => {
    expect(delta(112, 100)).toMatchObject({ dir: "up", text: "+12%" });
    expect(delta(96, 100)).toMatchObject({ dir: "down", text: "−4%" });
    expect(delta(100, 100)).toMatchObject({ dir: "flat", text: "0%" });
    expect(delta(1002, 1000)).toMatchObject({ dir: "flat", text: "0%" });
  });

  test("from nothing it is new, and nothing to nothing is flat", () => {
    expect(delta(5, 0)).toEqual({ pct: null, dir: "up", text: "new" });
    expect(delta(0, 0)).toMatchObject({ dir: "flat" });
  });

  test("periods split the tail from the stretch before it", () => {
    expect(periods([1, 2, 3, 4, 5], 2)).toEqual({ now: 9, before: 5 });
    expect(periods([4, 5], 3)).toEqual({ now: 9, before: 0 });
  });
});

describe("palette", () => {
  test("five fixed slots, and a sixth series is an error rather than a new hue", () => {
    expect(SERIES).toHaveLength(5);
    expect(seriesColor(0)).toBe(SERIES[0]);
    expect(() => seriesColor(5)).toThrow(/other/);
  });

  test("the upload kinds fit the slots, so a stack never runs out", () => {
    expect(UPLOAD_KINDS.length).toBeLessThanOrEqual(SERIES.length);
  });
});

describe("the sample data and the usage view", () => {
  const m = usageMock("2026-11-18", 60);

  test("the sample is fixed: same seed, same numbers", () => {
    expect(usageMock("2026-11-18", 60)).toEqual(m);
    expect(usageMock("2026-11-18", 60, 8)).not.toEqual(m);
    expect(m.days).toHaveLength(60);
    expect(m.days.at(-1)!.day).toBe("2026-11-18");
  });

  test("a range's charts cover exactly its days, each series a value a day", () => {
    const v = usageView(m, "2026-11-18", 14);
    expect(v.days).toEqual(dayRange("2026-11-18", 14));
    for (const s of [...v.upload, ...v.messages, ...v.people, ...v.events]) expect(s.values).toHaveLength(14);
    expect(v.upload.map((s) => s.key)).toEqual(UPLOAD_KINDS.map((k) => k.key));
  });

  test("tiles total the range and compare with the range before", () => {
    const v = usageView(m, "2026-11-18", 7);
    const last7 = m.days.slice(-7), prev7 = m.days.slice(-14, -7);
    const events = v.tiles.find((t) => t.key === "events")!;
    expect(events.value).toBe(last7.reduce((a, d) => a + d.events, 0));
    expect(events.delta).toEqual(delta(events.value, prev7.reduce((a, d) => a + d.events, 0)));
    expect(events.trend).toEqual(last7.map((d) => d.events));
    const msgs = v.tiles.find((t) => t.key === "messages")!;
    expect(msgs.value).toBe(last7.reduce((a, d) => a + messagesOf(d), 0));
    const up = v.tiles.find((t) => t.key === "uploaded")!;
    expect(up.bytes).toBe(true);
    expect(tileValue(up, bytes)).toMatch(/ (MB|GB)$/);
    expect(tileValue(events, bytes)).toBe(compact(events.value));
  });

  test("a range reaching before the data is zero there, not missing", () => {
    const v = usageView(usageMock("2026-11-18", 10), "2026-11-18", 30);
    expect(v.days).toHaveLength(30);
    expect(v.events[0]!.values.slice(0, 20).every((x) => x === 0)).toBe(true);
    expect(v.tiles.find((t) => t.key === "games")!.delta.text).toBe("new");
  });

  test("event types: the biggest first, the tail folded into one quiet other row", () => {
    const v = usageView(m, "2026-11-18", 30, 5);
    expect(v.eventTypes).toHaveLength(6);
    expect(v.eventTypes[0]!.label).toBe("GameFrame");
    const other = v.eventTypes.at(-1)!;
    expect(other).toMatchObject({ other: true, label: `${m.event_types.length - 5} other types` });
    expect(v.eventTypes.reduce((a, r) => a + r.value, 0)).toBe(m.event_types.reduce((a, t) => a + t.count, 0));
  });

  test("stored drops kinds with nothing stored (the relay) and totals the rest", () => {
    const v = usageView(m, "2026-11-18", 30);
    expect(v.stored.some((r) => r.key === "relay")).toBe(false);
    expect(v.storedTotal).toBe(Object.values(m.stored).reduce((a, b) => a + b, 0));
    expect(v.stored.map((r) => r.value)).toEqual([...v.stored.map((r) => r.value)].sort((a, b) => b - a));
  });

  test("an account's upload is the sum of its kinds", () => {
    const a = m.accounts[0]!;
    expect(uploaded(a)).toBe(a.bytes_up.events + a.bytes_up.frames + a.bytes_up.content + a.bytes_up.relay);
  });
});
