import { describe, expect, test } from "bun:test";
import { motdHeader, motdMessages, randomStart, stepIndex } from "../src/lib/motd";

describe("the message of the day as shown", () => {
  test("a blank or missing header shows nothing", () => {
    expect(motdHeader(null)).toBeNull();
    expect(motdHeader({ header: "   ", messages: ["m"] })).toBeNull();
    expect(motdHeader({ header: " Welcome to h2 live ", messages: [] })).toBe("Welcome to h2 live");
  });

  test("blank messages are dropped and the rest trimmed", () => {
    expect(motdMessages(undefined)).toEqual([]);
    expect(motdMessages({ header: "", messages: ["", " a ", "  ", "b"] })).toEqual(["a", "b"]);
  });

  test("stepping goes round the ring both ways", () => {
    expect(stepIndex(0, 1, 3)).toBe(1);
    expect(stepIndex(2, 1, 3)).toBe(0);
    expect(stepIndex(0, -1, 3)).toBe(2);
    expect(stepIndex(1, -4, 3)).toBe(0);
  });

  test("stepping copes with an index past a shrunk list, and an empty one", () => {
    expect(stepIndex(9, 1, 3)).toBe(0);
    expect(stepIndex(9, 0, 3)).toBe(2);
    expect(stepIndex(4, 1, 0)).toBe(0);
  });

  test("a random start is always in range", () => {
    expect(randomStart(0)).toBe(0);
    expect(randomStart(5, () => 0)).toBe(0);
    expect(randomStart(5, () => 0.999999)).toBe(4);
    expect(randomStart(5, () => 1)).toBe(4);
    for (let i = 0; i < 50; i++) {
      const n = randomStart(7);
      expect(n).toBeGreaterThanOrEqual(0);
      expect(n).toBeLessThan(7);
    }
  });
});
