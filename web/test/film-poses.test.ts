import { expect, test } from "bun:test";
import { FilmPoses } from "../src/api/film-poses";
import { applyFilmEffects } from "../src/api/film-fx";
import { sampleFilmEffects, sampleFilmWorld } from "../src/viewer/film-world";

function uint(n: number) {
  const bytes = [];
  while (n >= 128) { bytes.push(n % 128 + 128); n = Math.floor(n / 128); }
  return [...bytes, n];
}

interface Edit { id: number; op: number; lanes?: Record<number, number>; name?: string }
function payload(op: number, edits: Edit[], hill = [0]) {
  const bytes = [op, ...uint(edits.length)];
  let prev = 0;
  for (const e of edits) {
    bytes.push(...uint(4 * (e.id - prev) + e.op));
    prev = e.id;
    if (e.op === 2) continue;
    const lanes = Object.entries(e.lanes ?? {}).sort(([a], [b]) => +a - +b);
    const mask = lanes.reduce((n, [i]) => n + 2 ** Number(i), e.name === undefined ? 0 : 2 ** 26);
    bytes.push(...uint(mask));
    for (const [, v] of lanes) bytes.push(...uint(v < 0 ? -2 * v - 1 : 2 * v));
    if (e.name !== undefined) {
      const text = new TextEncoder().encode(e.name);
      bytes.push(...uint(text.length), ...text);
    }
  }
  bytes.push(...hill);
  if (op === 0) bytes.push(0);
  return Uint8Array.from(bytes);
}

test("independent Rust golden baseline identifies a loose weapon", () => {
  const p = new FilmPoses("g");
  p.apply(12, Uint8Array.of(0, 1, 4, 1, 4, 0, 0));
  expect(p.frame(12).items?.[0]?.id).toBe("00000001");
  expect(p.frame(12).film_world?.props[0]?.class).toBe("weap");
  p.apply(13, Uint8Array.of(31, 1, 2, 3));
  expect(p.frame(13).items).toEqual(p.frame(12).items);
});

test("parented seats, turret aim and projectiles use salted handles and centimetres", () => {
  const p = new FilmPoses("g");
  const x = -30.123, dir = 2.74, id = 0x81230000;
  const yaw = Math.round(dir * 65536 / (2 * Math.PI));
  p.apply(1, payload(0, [
    { id, op: 0, lanes: {0: 1, 5: Math.round(x * 304.8)}, name: "warthog" },
    { id: id + 1, op: 0, lanes: {4: id + 1, 5: 305, 12: 3, 22: 0x81230005} },
    { id: id + 2, op: 0, lanes: {0: 1, 4: id + 1, 13: 1, 14: yaw}, name: "turret" },
    { id: id + 3, op: 0, lanes: {0: 5, 4: id + 2, 7: 305}, name: "plasma_grenade" },
  ], [2, 4, 1, 2, 4, 6]));
  const frame = p.frame(1);
  const hull = frame.vehicles![0]!;
  expect(hull.occupied).toBe(true);
  expect(Math.abs(hull.pos[0] - x) * 304.8).toBeLessThanOrEqual(.5);
  expect(frame.players[0]).toMatchObject({ index: 4, seat: 2, vehicle: hull.id });
  const turret = frame.vehicles![1]!;
  expect(turret.parent).toBe(hull.id);
  expect(Math.abs(Math.atan2(turret.aim![1], turret.aim![0]) - dir)).toBeLessThanOrEqual(Math.PI / 65536);
  expect(frame.projectiles![0]!.pos[0]).toBeCloseTo(hull.pos[0] + 305 / 304.8, 8);
  expect(frame.projectiles![0]!.pos[2]).toBeCloseTo(305 / 304.8, 8);
  expect(frame.film_world!.hill!.index).toBe(4);
});

test("rest costs no delta, destruction and respawn take effect at their own tick", () => {
  const p = new FilmPoses("g");
  const first = payload(0, [{ id: 9, op: 0, lanes: { 0: 11 }, name: "crate" }]);
  p.apply(1, first);
  const a = p.frame(1);
  p.apply(4, payload(1, [{ id: 9, op: 1, lanes: {5: 305} }]));
  const b = p.frame(4);
  p.apply(7, payload(1, [{ id: 9, op: 2 }, { id: 65545, op: 0, lanes: {0: 11} }]));
  const c = p.frame(7);
  expect(sampleFilmWorld([a, b, c], 2.5)!.props[0]!.pos[0]).toBeCloseTo(152.5 / 304.8, 6);
  expect(sampleFilmWorld([a, b, c], 6.75)!.props[0]!.id).toBe("00000009");
  expect(sampleFilmWorld([a, b, c], 7)!.props[0]!.id).toBe("00010009");
  p.apply(1, first);
  expect(p.frame(1)).toEqual(a);
});

test("a complete census preserves placement, held items, machines and device travel", () => {
  const p = new FilmPoses("g");
  p.apply(1, payload(0, [
    { id: 1, op: 0, lanes: { 0: 11, 2: 12 }, name: "crate" },
    { id: 2, op: 0, lanes: { 0: 7, 2: 3, 20: 129 }, name: "wheel" },
    { id: 3, op: 0, lanes: { 22: 1 } },
    { id: 4, op: 0, lanes: { 0: 2, 3: 4, 4: 4, 23: 1 } },
  ]));
  const f = p.frame(1);
  expect(f.objects).toContainEqual(expect.objectContaining({ k: "netgame", i: 3, gone: true }));
  expect(f.objects![0]!.k).toBe("bloc");
  expect(f.machines![0]!.node).toBe("wheel_2");
  expect(f.machine_positions).toEqual([{i: 2, p: 128}]);
  expect(f.players[0]!.weapon).toBe("flag");
});

test("truncation, missing dictionaries and invalid edit targets fail decoding", () => {
  const p = new FilmPoses("g");
  expect(() => p.apply(4, payload(1, []))).toThrow();
  const key = payload(0, [{ id: 8, op: 0, lanes: {0: 5}, name: "rocket" }]);
  for (let n = 0; n < key.length; ++n) {
    expect(() => new FilmPoses("g").apply(1, key.subarray(0, n))).toThrow();
  }
  p.apply(1, key);
  expect(() => p.apply(2, payload(1, [{ id: 9, op: 2 }]))).toThrow();
  const before = p.frame(1);
  expect(() => p.apply(2, payload(1, [{ id: 8, op: 2 }]).subarray(0, -1))).toThrow();
  expect(p.frame(1)).toEqual(before);
});

test("recorded hits, muzzle geometry, blast centres and final points have independent lifetimes", () => {
  const poses = new FilmPoses("g");
  poses.apply(1, payload(0, [{ id: 99, op: 0, lanes: {0: 1, 5: 3048} }, { id: 101, op: 0, lanes: {22: 1} }]));
  const frame = poses.frame(50);
  const hit = {kind: "Impact", age: 1, data: {code: 15, target: 101, source: 99, effect: 7, origin: [3, 4, 5], direction: [1, 0, 0], distance: 10}};
  applyFilmEffects(frame, [
    hit, {...hit, data: {...hit.data, target: 102}},
    {kind: "WeaponFire", age: 1, data: {unit: "p0", count: 1, barrel: {parent: 99, origin: [1, 2, 3], aim: [1, 0, 0]}}},
    {kind: "ProjectileEnded", age: 0, data: {projectile: 15, parent: 99, position: [2, 0, 0], velocity: [12, 0, 0]}},
    {kind: "Damage", age: 1, data: {pos: [5, 4, 3], from: "p0", to: "p1", amount: 0.5}},
    {kind: "Pickup", age: 0, data: {unit: "p0"}},
  ], poses);
  expect(frame.players[0]).toMatchObject({ shots: 1, fire: 2, actions: ["pickup"] });
  expect(frame.hits?.[0]).toMatchObject({ pos: [5, 4, 3], ago: 1 });
  expect(frame.film_effects?.filter(e => e.kind === "blast")).toHaveLength(1);
  expect(frame.film_effects?.[0]?.pos).toEqual([3, 4, 5]);
  expect(frame.film_effects?.[1]?.pos[0]).toBeCloseTo(11);
  expect(frame.film_effects?.[2]?.pos[0]).toBeCloseTo(12);
  const frames = [frame];
  expect(sampleFilmEffects(frames, 48)).toHaveLength(0);
  expect(sampleFilmEffects(frames, 49)).toHaveLength(2);
  expect(sampleFilmEffects(frames, 51)).toHaveLength(3);
  expect(sampleFilmEffects(frames, 58)).toHaveLength(1);
  expect(sampleFilmEffects(frames, 61)).toHaveLength(0);
});
