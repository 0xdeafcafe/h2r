import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { parseModels } from "../src/viewer/models";
import { BEAM_MS, KIND_LOOKS, Projectiles, flown, lookFor, shotEnd } from "../src/viewer/projectiles";

describe("what a shot looks like", () => {
  test("by weapon, with the disc's speeds where it has them", () => {
    expect(lookFor("battle_rifle", "rifle").kind).toBe("bullet");
    expect(lookFor("plasma_pistol", "pistol").colour).toBe(0x5cff5a);
    expect(lookFor("needler", "pistol").kind).toBe("needle");
    expect(lookFor("rocket_launcher", "missile").kind).toBe("rocket");
    expect(lookFor("beam_rifle", "rifle").kind).toBe("beam");
    // A sword swings; so does anything held in the sword class.
    expect(lookFor("energy_blade", "sword").kind).toBe("melee");
    expect(lookFor("something_new", "sword").kind).toBe("melee");
    // Unknown: a bullet, unless the disc says otherwise.
    expect(lookFor("something_new", "rifle").kind).toBe("bullet");
    expect(lookFor("something_new", "rifle", { kind: "plasma", speed: 12 }).kind).toBe("plasma");
    const pr = lookFor("plasma_rifle", "rifle", { kind: "plasma", speed: 14 });
    expect([pr.speed, pr.finalSpeed]).toEqual([14, 14]);
    const rocket = lookFor("rocket_launcher", "missile", { kind: "rocket", speed: 8, final_speed: 20 });
    expect([rocket.speed, rocket.finalSpeed]).toEqual([8, 20]);
    // A beam is instant whatever the tag says; nonsense speeds are ignored.
    expect(lookFor("beam_rifle", "rifle", { speed: 1200 }).speed).toBe(Infinity);
    expect(lookFor("smg", "rifle", { speed: -3 }).speed).toBe(KIND_LOOKS.bullet.speed);
    expect(lookFor(null, null).kind).toBe("bullet");
  });

  test("a rocket speeds up over its first second, then holds", () => {
    const r = { speed: 8, finalSpeed: 20 };
    expect(flown(r, 0)).toBe(0);
    expect(flown(r, 1)).toBeCloseTo(14, 9);
    expect(flown(r, 2)).toBeCloseTo(34, 9);
    expect(flown({ speed: 14, finalSpeed: 14 }, 0.5)).toBeCloseTo(7, 9);
    expect(flown({ speed: Infinity, finalSpeed: Infinity }, 0.01)).toBe(Infinity);
  });

  test("a shot ends at its victim, else at the level, else at its range", () => {
    const o = new THREE.Vector3(0, 0, 0), dir = new THREE.Vector3(1, 0, 0);
    expect(shotEnd(o, dir, 60, new THREE.Vector3(10, 0.5, 0), null)).toBeCloseTo(Math.hypot(10, 0.5), 9);
    // A victim behind the shooter is another shot's.
    expect(shotEnd(o, dir, 60, new THREE.Vector3(-10, 0, 0), () => 25)).toBe(25);
    expect(shotEnd(o, dir, 60, null, () => 25)).toBe(25);
    expect(shotEnd(o, dir, 60, null, () => null)).toBe(60);
    expect(shotEnd(o, dir, 60, null, null)).toBe(60);
  });

  test("the manifest's per-weapon ballistics are read, bad values dropped", () => {
    const m = parseModels({ version: 2, bipeds: {}, weapons: {
      plasma_rifle: { glb: "models/weapon/plasma_rifle.glb", class: "rifle", ballistics: { kind: "plasma", speed: 14, rate: 9 } },
      smg: { glb: "models/weapon/smg.glb", ballistics: { kind: "BULLET!", speed: "fast" } },
    } })!;
    expect(m.ballistics!.plasma_rifle).toEqual({ kind: "plasma", speed: 14, final_speed: undefined, rate: 9 });
    expect(m.ballistics!.smg).toEqual({ kind: undefined, speed: undefined, final_speed: undefined, rate: undefined });
  });
});

describe("shots in flight", () => {
  const meshes = (scene: THREE.Scene) => scene.children.filter((c) => c.visible) as THREE.Mesh[];

  test("a plasma bolt travels at its speed and is gone when it arrives", () => {
    const scene = new THREE.Scene(), p = new Projectiles(scene);
    p.fire(lookFor("plasma_rifle", "rifle", { speed: 10 }), new THREE.Vector3(0, 1, 0), new THREE.Vector3(1, 0, 0), 5, 0);
    expect(p.update(250)).toBe(true);
    const bolt = meshes(scene).find((m) => m.geometry.type === "SphereGeometry")!;
    expect(bolt.position.x).toBeCloseTo(2.5, 6);
    expect(bolt.position.y).toBeCloseTo(1, 6);
    expect(p.update(600)).toBe(false);
    expect(meshes(scene).length).toBe(0);
    // Its meshes are kept for the next one.
    p.fire(lookFor("plasma_rifle", "rifle", { speed: 10 }), new THREE.Vector3(), new THREE.Vector3(1, 0, 0), 5, 1000);
    expect(scene.children.length).toBe(2);
  });

  test("a tracer's streak runs to the end and its tail catches up", () => {
    const scene = new THREE.Scene(), p = new Projectiles(scene);
    const look = lookFor("smg", "rifle", { speed: 400 });
    p.fire(look, new THREE.Vector3(), new THREE.Vector3(1, 0, 0), 20, 0);
    p.update(25); // 10 units out
    const streak = meshes(scene)[0]!;
    expect(streak.position.x).toBeCloseTo(10, 6);
    expect(streak.scale.x).toBeCloseTo(look.length, 6);
    expect(p.update(55)).toBe(true); // head at the end, tail closing in
    expect(streak.position.x).toBeCloseTo(20, 6);
    expect(p.update(60)).toBe(false);
  });

  test("a beam is the whole line at once, fading; a sword fires nothing", () => {
    const scene = new THREE.Scene(), p = new Projectiles(scene);
    p.fire(lookFor("beam_rifle", "rifle"), new THREE.Vector3(), new THREE.Vector3(0, 0, -1), 30, 0);
    p.update(10);
    const beam = meshes(scene)[0]!;
    expect(beam.scale.x).toBe(30);
    expect(beam.position.z).toBeCloseTo(-30, 6);
    expect(p.update(BEAM_MS + 1)).toBe(false);
    p.fire(lookFor("energy_blade", "sword"), new THREE.Vector3(), new THREE.Vector3(1, 0, 0), 30, 0);
    expect(p.count).toBe(0);
  });
});
