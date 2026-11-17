import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { sampleVehicles } from "../src/state/frames";
import type { Frame } from "../src/api/types";
import { PITCH_DOWN, PITCH_UP, easeAngle, mountedTurrets, pickTurret, turretAngles } from "../src/viewer/turrets";
import { findTurrets, poseTurret, turretWant } from "../src/viewer/turrets3d";
import { gameToGl, glToGame } from "../src/map/views";

describe("turret aim maths", () => {
  test("level ahead is nothing; left is a quarter yaw; up is pitch", () => {
    expect(turretAngles([1, 0, 0])).toEqual({ yaw: 0, pitch: 0 });
    expect(turretAngles([0, 1, 0]).yaw).toBeCloseTo(Math.PI / 2, 9);
    expect(turretAngles([0, -1, 0]).yaw).toBeCloseTo(-Math.PI / 2, 9);
    const up = turretAngles([1, 0, 1]);
    expect(up.yaw).toBe(0);
    expect(up.pitch).toBeCloseTo(Math.PI / 4, 9);
  });
  test("pitch is clamped; straight up keeps yaw; nonsense is rest", () => {
    expect(turretAngles([0, 0, 1])).toEqual({ yaw: 0, pitch: PITCH_UP });
    expect(turretAngles([1, 0, -100]).pitch).toBe(-PITCH_DOWN);
    expect(turretAngles([0, 0, 0])).toEqual({ yaw: 0, pitch: 0 });
    expect(turretAngles([NaN, 0, 0])).toEqual({ yaw: 0, pitch: 0 });
  });
  test("easing takes the short way round and lands in the same place at any frame rate", () => {
    // From just left of behind to just right of behind: through behind, not ahead.
    const a = easeAngle(3.0, -3.0, 0.016, 0.08);
    expect(Math.abs(a)).toBeGreaterThan(3.0);
    let x = 0, y = 0;
    for (let i = 0; i < 60; i++) x = easeAngle(x, 1, 1 / 60, 0.1);
    for (let i = 0; i < 30; i++) y = easeAngle(y, 1, 1 / 30, 0.1);
    expect(x).toBeCloseTo(y, 6);
    expect(easeAngle(0, 2, 0.016, 0)).toBe(2);
  });
});

describe("which vehicle a turret belongs to", () => {
  const hog = { id: "a", kind: "warthog" };
  const gun = { id: "b", kind: "chaingun", parent: "a", aim: [0, 1, 0] as [number, number, number] };
  test("a turret's own aim, under its vehicle; its gunner rides the vehicle", () => {
    const m = mountedTurrets([hog, gun], [{ vehicle: "b", aim: [1, 0, 0], alive: true }]);
    expect(m.hullOf.get("b")).toBe("a");
    expect(m.aims.get("a")).toEqual([{ kind: "chaingun", aim: [0, 1, 0] }]);
  });
  test("without its own aim, the living gunner's", () => {
    const m = mountedTurrets([hog, { ...gun, aim: undefined }], [
      { vehicle: "b", aim: [0, 0, 1], alive: false },
      { vehicle: "b", aim: [1, 0, 0], alive: true },
    ]);
    expect(m.aims.get("a")).toEqual([{ kind: "chaingun", aim: [1, 0, 0] }]);
  });
  test("no parent in the list, or a turret on a turret: left out", () => {
    expect(mountedTurrets([gun], []).hullOf.size).toBe(0);
    const m = mountedTurrets([hog, gun, { id: "c", kind: "x", parent: "b", aim: [1, 0, 0] }], []);
    expect(m.hullOf.has("c")).toBe(false);
  });
  test("a model's turret by its tag, else its only one", () => {
    expect(pickTurret(["mortar_mp", "minigun"], "minigun")).toBe(1);
    expect(pickTurret(["cannon_mp"], "cannon")).toBe(0);
    expect(pickTurret(["a", "b"], "c")).toBe(-1);
    expect(pickTurret([], "c")).toBe(-1);
  });
});

describe("a turret in the frames", () => {
  const frame = (tick: number, x: number, aim: [number, number, number]): Frame => ({
    game_id: "g", tick, players: [],
    vehicles: [
      { id: "90040004", kind: "warthog", pos: [x, 5, 1], forward: [1, 0, 0], up: [0, 0, 1] },
      { id: "90080008", kind: "chaingun", pos: [-0.6, 0, 0.75], forward: [1, 0, 0], up: [0, 0, 1], parent: "90040004", aim },
    ],
  });
  test("is sampled where its vehicle is, its aim eased between frames", () => {
    const v = sampleVehicles([frame(0, 10, [1, 0, 0]), frame(2, 12, [0, 1, 0])], 1);
    const gun = v.find((q) => q.kind === "chaingun")!;
    expect(gun.parent).toBe("90040004");
    expect([gun.x, gun.y, gun.z]).toEqual([11, 5, 1]);
    expect(gun.aim![0]).toBeCloseTo(Math.SQRT1_2, 6);
    expect(gun.aim![1]).toBeCloseTo(Math.SQRT1_2, 6);
    expect(v.find((q) => q.kind === "warthog")!.parent).toBeUndefined();
  });
});

describe("posing a turret model", () => {
  /** A vehicle glb's shape: root, the turret part at the turret marker
   *  (behind and above the middle), its gun at the gun's pivot. */
  const model = () => {
    const root = new THREE.Group();
    const turret = new THREE.Group();
    turret.name = "turret_0";
    turret.userData.turret = "chaingun";
    turret.position.set(...gameToGl(-0.6, 0, 0.75));
    const gun = new THREE.Group();
    gun.name = "turret_0_pitch_0";
    gun.position.set(...gameToGl(0.3, 0, 0.4));
    const barrel = new THREE.Object3D();
    barrel.position.set(1, 0, 0); // one unit along the gun's +x (game forward)
    gun.add(barrel);
    turret.add(gun);
    root.add(turret);
    return { root, barrel };
  };
  const pointing = (root: THREE.Object3D, barrel: THREE.Object3D) => {
    root.updateMatrixWorld(true);
    const gun = barrel.parent!;
    const a = gun.getWorldPosition(new THREE.Vector3()), b = barrel.getWorldPosition(new THREE.Vector3());
    const d = b.sub(a).normalize();
    return glToGame(d.x, d.y, d.z);
  };

  test("finds the turret and its gun by name, with its tag", () => {
    const t = findTurrets(model().root);
    expect(t.length).toBe(1);
    expect(t[0]!.kind).toBe("chaingun");
    expect(t[0]!.guns.length).toBe(1);
  });

  for (const [name, aim] of [["left and up", [0, 1, 1]], ["behind and down", [-1, 0, -0.3]], ["ahead", [1, 0, 0]]] as const) {
    test(`the gun points where the aim does: ${name}`, () => {
      const { root, barrel } = model();
      const t = findTurrets(root)[0]!;
      const l = Math.hypot(...aim), want = aim.map((c) => c / l);
      t.now = turretWant(t, want as [number, number, number]);
      poseTurret(t);
      const got = pointing(root, barrel);
      for (let k = 0; k < 3; k++) expect(got[k]).toBeCloseTo(want[k]!, 5);
    });
  }

  test("on a vehicle turned and tilted, the aim is still the world's", () => {
    const { root, barrel } = model();
    const car = new THREE.Group();
    car.add(root);
    car.quaternion.setFromEuler(new THREE.Euler(0.2, 1.1, -0.1));
    car.scale.setScalar(0.3);
    const t = findTurrets(root)[0]!;
    const want: [number, number, number] = [0.6, -0.48, 0.64];
    t.now = turretWant(t, want);
    poseTurret(t);
    car.updateMatrixWorld(true);
    const got = pointing(car, barrel);
    for (let k = 0; k < 3; k++) expect(got[k]).toBeCloseTo(want[k]!, 5);
  });
});
