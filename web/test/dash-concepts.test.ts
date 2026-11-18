import { expect, test } from "bun:test";
import { horizontalNeighbor } from "../src/dash/concepts/controls";
import { online, peak, players, sampleTime, screenFrom, traffic } from "../src/dash/concepts/model";

test("comparison links accept each screen and default stale links to home", () => {
  for (const id of ["friends","messages","sessions","account","server"] as const) expect(screenFrom(id)).toBe(id);
  for (const invalid of [undefined, ["account","server"], "toString", "other"]) expect(screenFrom(invalid)).toBe("home");
});

test("edge navigation stays on the control, side navigation favors its row", () => {
  const points = [{ x: 25, y: 60 }, { x: 85, y: 155 }, { x: 150, y: 59 }, { x: 170, y: 30 }];
  expect(horizontalNeighbor(points, 0, 1)).toBe(2);
  expect(horizontalNeighbor(points, 0, -1)).toBe(0);
  expect(horizontalNeighbor(points, 2, -1)).toBe(0);
});

test("presence and chart summaries match the fixtures and their sample times", () => {
  expect(online.map(f => f.tag)).toEqual(["Kestrel","Mossfire","Halcyon"]);
  expect(sampleTime(traffic.indexOf(peak))).toBe("15:30");
  expect(sampleTime(traffic.length-1)).toBe("18:00");
  expect(players).toBe(42);
});

test("the finish follows dashboard navigation and leaves other destinations alone", async () => {
  const { readFinish, finishHref } = await import("../src/dash/concepts/finish");
  for (const finish of ["original", "refined"] as const) {
    expect(readFinish(finish)).toBe(finish);
    expect(finishHref("/", finish)).toBe("/?finish=" + finish);
    expect(finishHref("/dash/signin?source=menu#account", finish)).toBe("/dash/signin?source=menu&finish=" + finish + "#account");
    expect(finishHref("/stats", finish)).toBe("/stats");
  }
  expect(finishHref("/dash/signin", readFinish("unknown"))).toBe("/dash/signin");
});
