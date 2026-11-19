// The MP fixture never reads a pad. With a real trace, supply raw packed
// XINPUT_GAMEPAD (18 bytes per player per tick, LE, tick-major), no sequence
// counter. Estimate each channel's held-value residual, separately per port.
// bun web/bench/live-floor/inputs.mjs [pad.bin players]
import { readFileSync } from "node:fs";
function binaryH(p) {
  return -p * Math.log2(p) - (1 - p) * Math.log2(1 - p);
}
function entropy(h) {
  const n = [...h.values()].reduce((a, b) => a + b, 0);
  let bits = 0;
  for (const v of h.values()) bits += v * Math.log2(n / v);
  return bits;
}
function bump(m, k) {
  m.set(k, (m.get(k) ?? 0) + 1);
}
const bytesPerReport = 18,
  hz = 30;
const model = (n, p, innovationBits) => n * (binaryH(p) + p * innovationBits);
const hypothetical = model(4, 0.25, 4) + model(8, 0.01, 0) + model(8, 0.05, 5);
console.log("Uncompressed pad B/player/s", bytesPerReport * hz);
console.log(
  "Illustration only, independent channels: 4 axes change on 25% of ticks, 4 innovation bits/change; 8 digital bits flip on 1%; 8 pressures change on 5%, 5 innovation bits/change. B/player/s",
  (hypothetical * hz) / 8,
);
if (process.argv[2]) {
  const bytes = readFileSync(process.argv[2]),
    players = Number(process.argv[3] ?? 1);
  if (
    !Number.isInteger(players) ||
    players <= 0 ||
    bytes.length % (18 * players)
  )
    throw Error("incomplete tick or invalid player count");
  const ticks = bytes.length / (18 * players);
  if (ticks < 3) throw Error("need at least three ticks");
  const totals = [0, 0];
  for (let p = 0; p < players; p++)
    for (let ch = 0; ch < 13; ch++) {
      const current = new Map(),
        from = new Map(),
        pairs = new Map();
      let old, previous;
      for (let i = 0; i < ticks; i++) {
        const at = (i * players + p) * 18;
        const value =
          ch === 0
            ? bytes.readUInt16LE(at)
            : ch <= 8
              ? bytes[at + 1 + ch]
              : bytes.readInt16LE(at + 10 + (ch - 9) * 2);
        if (old !== undefined) {
          const r = ch === 0 ? old ^ value : value - old;
          bump(current, r);
          if (previous !== undefined) {
            bump(from, previous);
            bump(pairs, previous + "," + r);
          }
          previous = r;
        }
        old = value;
      }
      totals[0] += entropy(current);
      totals[1] += entropy(pairs) - entropy(from);
    }
  console.log({
    players,
    ticks,
    in_sample_marginal_H0_B_player_s:
      (totals[0] * hz) / (8 * players * (ticks - 1)),
    in_sample_marginal_H1_B_player_s:
      (totals[1] * hz) / (8 * players * (ticks - 2)),
  });
}
