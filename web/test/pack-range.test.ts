import { expect, test } from "bun:test";
import { PackError } from "../src/api/pack";
import { RangeModels, RangeReader } from "../src/api/pack-range";

// One bit for odd/even followed by the unsigned value, sharing the same
// contexts over two packets. The second starts with the first's counts.
const packets = [
  ["00314380fda3ea2e1875a8be2c72a76000", [0, 7, 8, 31, 513, 1023, 1024, 432123]],
  ["00bdd9a503b6bf16eb73119000", [3, 0, 0, 257, 2 ** 32 + 47]],
] as const;

test("v8 byte symbols decode across packet boundaries and integer escape sizes", () => {
  const models = new RangeModels(64);
  for (const [hex, values] of packets) {
    const r = new RangeReader(Buffer.from(hex, "hex"), models);
    for (const value of values) {
      expect(r.bit(0)).toBe(value % 2 === 1);
      expect(r.uint(32)).toBe(value);
    }
    r.finish();
  }
});

test("a range record stays inside its envelope and rejects a noncanonical ending", () => {
  const [hex, values] = packets[0];
  const bytes = Buffer.from("ff" + hex + "00", "hex");
  const read = (end: number) => {
    const r = new RangeReader(bytes, new RangeModels(64), 1, end);
    for (const _ of values) { r.bit(0); r.uint(32); }
    r.finish();
  };
  expect(() => read(bytes.length - 1)).not.toThrow();
  expect(() => read(bytes.length - 2)).toThrow(PackError);
  expect(() => read(bytes.length)).toThrow(PackError);
  expect(() => new RangeReader(bytes, new RangeModels(64))).toThrow(PackError);
});
