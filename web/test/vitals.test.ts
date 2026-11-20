import { describe, expect, test } from "bun:test";
import { DAMAGE_KEY, VitalsTrack, damagePref, saveDamagePref, vitalsNumber } from "../src/map/vitals";
import { actorIndex, type SampledHit, type SampledPlayer } from "../src/state/frames";

const victim = (body?: number, shield?: number, vitalsTick?: number, alive = true): SampledPlayer =>
  ({ index: 0, x: 0, y: 0, z: 0, yaw: 0, vehicle: null, alive, body, shield, vitalsTick });
const attacker: SampledPlayer = { ...victim(), index: 1, x: 2, y: 4, z: 1 };
const hit: SampledHit = { from: "p1", to: "p0", weapon: "smg", kind: "bullet", x: 0, y: 0, z: 0, amount: 42 };

function sample(track: VitalsTrack, now: number, body: number, shield: number, opts?: { reducedMotion?: boolean }) {
  return track.observe({ players: [victim(body, shield)] }, now, opts).get(0)!;
}

describe("vitals", () => {
  test("drops 500 ms apart join, then a drop 700 ms later starts another burst", () => {
    const track = new VitalsTrack();
    sample(track, 0, 1, 1);
    const first = sample(track, 100, 1, 0.75);
    expect(first).toMatchObject({ shieldChip: 1, bodyChip: null, chipFrom: 2, lost: 12.5 });
    const joined = sample(track, 600, 1, 0.5);
    expect(joined).toMatchObject({ shieldChip: 1, chipFrom: 2, lost: 25 });
    const second = sample(track, 1300, 1, 0.25);
    expect(second).toMatchObject({ shieldChip: 0.5, chipFrom: 1.5, lost: 12.5 });
  });

  test("a kill from full costs 100 and damage through overshield can exceed it", () => {
    const track = new VitalsTrack();
    sample(track, 0, 1, 1);
    const killed = track.observe({ players: [victim(0, 0, 3, false)] }, 100).get(0)!;
    expect(killed.lost).toBe(100);
    expect(vitalsNumber(killed)).toBe("100");
    expect(killed.state).not.toBe("broken");

    track.clear();
    sample(track, 0, 1, 2);
    expect(sample(track, 100, 0, 0).lost).toBe(150);
  });

  test("shield break, recharge and overshield follow the sampled vitality", () => {
    const track = new VitalsTrack();
    expect(sample(track, 0, 1, 1).state).toBe("steady");
    expect(sample(track, 40, 1, 0.25).state).toBe("hit");
    expect(sample(track, 80, 1, 0)).toMatchObject({ state: "broken", breaking: true });
    expect(sample(track, 170, 1, 0).breaking).toBe(false);
    expect(sample(track, 260, 1, 0).breaking).toBe(true);
    expect(sample(track, 350, 1, 0)).toMatchObject({ state: "broken", breaking: false });
    expect(sample(track, 400, 1, 0.1).state).toBe("recharging");
    expect(sample(track, 500, 1, 0.1).state).toBe("recharging");
    expect(sample(track, 1800, 1, 1).state).toBe("steady");
    expect(sample(track, 2000, 1, 1.25).state).toBe("over");
  });

  test("a drop uses a hit within three ticks, using the hit's own tick and captured attacker position", () => {
    const track = new VitalsTrack();
    track.observe({ players: [victim(1, 1, 0), attacker] }, 0);
    track.observe({ players: [victim(1, 1, 8), attacker], hits: { tick: 8, hits: [{ ...hit, ago: 1 }] } }, 280);
    const matched = track.observe({ players: [victim(1, 0.5, 10), { ...attacker, x: 30 }] }, 330).get(0)!;
    expect(matched).toMatchObject({ from: { x: 2, y: 4, z: 1 }, kind: "bullet", lost: 25 });
    const noHit = track.observe({ players: [victim(1, 0.25, 11), attacker] }, 360).get(0)!;
    expect(noHit).toMatchObject({ from: null, kind: null, lost: 37.5 });
  });

  test("only observed bipeds get bars, including AI, and a quiet sample resets the burst", () => {
    const track = new VitalsTrack();
    const id = actorIndex("guard");
    const guard: SampledPlayer = { ...victim(1, 0, 1), index: id, actor: { id: "guard", team: null } };
    const baseline = track.observe({ players: [victim(undefined, 1)], actors: [guard] }, 0);
    expect(baseline.size).toBe(1);
    expect(baseline.get(id)).toMatchObject({ state: "steady", showBody: false });
    const next = { players: [victim()], actors: [{ ...guard, body: 0.5, vitalsTick: 2 }] };
    expect(track.observe(next, 50).get(id)).toMatchObject({ chipFrom: 1, lost: 25, showBody: true });
    expect(track.observe(next, 50, { quiet: true }).get(id)).toMatchObject({ chipFrom: null, lost: 0, state: "steady" });
    expect(track.observe({ players: [victim()] }, 100).size).toBe(0);
  });

  test("the chip holds, drains and clears with its number, while reduced motion snaps it off", () => {
    const track = new VitalsTrack();
    sample(track, 0, 1, 1);
    expect(sample(track, 100, 0.5, 0)).toMatchObject({ shieldChip: 1, bodyChip: 1, flare: true });
    expect(sample(track, 220, 0.5, 0).flare).toBe(false);
    expect(sample(track, 1050, 0.5, 0)).toMatchObject({ shieldChip: 1, bodyChip: 1 });
    expect(sample(track, 1175, 0.5, 0)).toMatchObject({ shieldChip: 0.5, bodyChip: 0.75 });
    expect(vitalsNumber(sample(track, 1300, 0.5, 0))).toBe("");

    track.clear();
    const reduced = { reducedMotion: true };
    sample(track, 0, 1, 1, reduced);
    expect(sample(track, 100, 1, 0, reduced)).toMatchObject({ shieldChip: 1, breaking: true });
    expect(sample(track, 399, 1, 0, reduced).breaking).toBe(true);
    expect(sample(track, 400, 1, 0, reduced).breaking).toBe(false);
    expect(sample(track, 700, 1, 0, reduced).shieldChip).toBeNull();
  });
});

test("damage preference reads the address before storage and defaults to bars", () => {
  const store = { getItem: (key: string) => key === DAMAGE_KEY ? "numbers" : null };
  expect(damagePref("off", store)).toBe("off");
  expect(damagePref(null, store)).toBe("numbers");
  expect(damagePref("garbage", store)).toBe("numbers");
  expect(damagePref(undefined, null)).toBe("bars");
  let saved: string[] | undefined;
  saveDamagePref("bars", { setItem: (key, value) => { saved = [key, value]; } });
  expect(saved).toEqual([DAMAGE_KEY, "bars"]);
});
