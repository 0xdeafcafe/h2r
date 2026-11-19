import { test } from "bun:test";
import assert from "node:assert/strict";
import { Connection } from "./stream.mjs";

test("packets decode sequentially, across losses at a key, and from a sender snapshot", () => {
  const enc = new Connection(),
    dec = new Connection();
  const first = {
    game_id: "trial",
    tick: 1,
    players: [],
    objects: [],
    hits: [],
  };
  const p = {
    index: 0,
    alive: true,
    pos: [1 / 1024, -2, 3],
    aim: [0, 0, 1],
    yaw: 0,
  };
  const rows = [
    first,
    { ...first, tick: 3, players: [p], hits: [{ pos: p.pos }] },
    { ...first, tick: 8, players: [{ ...p, actions: ["reload"], shots: 3 }] },
    { ...first, tick: 11, cut: 11, players: [p] },
    { ...first, tick: 14 },
  ];
  let joined;
  for (const [i, row] of rows.entries()) {
    if (i === 2) {
      joined = new Connection();
      joined.resume(enc.snapshot());
    }
    const packet = enc.encode(row, i === 0 || i === 3);
    assert.deepStrictEqual(dec.decode(packet), row);
    if (joined) assert.deepStrictEqual(joined.decode(packet), row);
    if (i === 2)
      assert.throws(() => new Connection().decode(packet), /snapshot/);
    if (i === 3) assert.deepStrictEqual(new Connection().decode(packet), row);
    assert.throws(
      () => new Connection().decode(packet.subarray(0, -1)),
      /truncated/,
    );
  }
});

test("column state preserves fractional literals, septet boundaries, signs and optional presence", () => {
  const enc = new Connection(),
    dec = new Connection();
  for (let i = 0; i < 512; i++) {
    const row = {
      game_id: "test",
      tick: i,
      players: [],
      probe: [i - 256, i % 4 ? null : -0, Math.PI * i],
      machine_positions: i % 3 ? [{ i: 32767, p: i % 256 }] : [],
    };
    if (i % 2) row.cinematic = false;
    assert.deepStrictEqual(dec.decode(enc.encode(row, i % 127 === 0)), row);
  }
});
