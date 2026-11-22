import { describe, expect, test } from "bun:test";
import { killFeedRows, type FeedKill } from "../src/components/killFeedRows";

const kill = (position: number, tick = position): FeedKill => ({
  position, tick, victim: 1, killer: 0, weapon: null, kind: "kill",
});

describe("Field Report row keys", () => {
  test("rebuilt replay events and prepended kills keep existing row identities", () => {
    const first = [kill(31), kill(32)];
    const keys = killFeedRows(first, 12).map((row) => row.key);
    const next = [...first.map((event) => ({ ...event, weapon: "rifle" })), kill(33)];
    expect(killFeedRows(next, 12).map((row) => row.key)).toEqual(["event/33", ...keys]);
    expect(killFeedRows(next, 2).map((row) => row.key)).toEqual(["event/33", keys[0]!]);
  });

  test("same-tick legacy duplicates keep distinct, stable keys as the feed rolls", () => {
    const first = [kill(0, 30), kill(0, 30)];
    const keys = killFeedRows(first, 2).map((row) => row.key);
    expect(new Set(keys).size).toBe(2);
    const next = [...first, kill(0, 31)];
    expect(killFeedRows(next, 2)[1]?.key).toBe(keys[0]);
  });
});
