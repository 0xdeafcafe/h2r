// Players and AI as the game had them: on the game's curved path between
// samples (Hermite, from the frames' velocities), feet on the floor while
// the game has them standing, the game's own animation at its frame, and
// riders in their seats.

import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import type { Frame, FramePlayer } from "../src/api/types";
import { HERMITE_MAX_TICKS, blendAnim, cleanAnim, cleanSeat, cleanVel, hermite, sampleActors, sampleFrames, type SampledPlayer } from "../src/state/frames";
import { clipFor, clipTime, gameAnim, sideways } from "../src/viewer/anim";
import { GROUND_BUDGET, GROUND_DOWN, GROUND_UP, GroundCache, floorAt, footZ, onFoot, type Floor } from "../src/viewer/ground";
import { parseModels, seatNodeFor, seatState } from "../src/viewer/models";
import { ModelLibrary, prepareBiped } from "../src/viewer/players3d";
import { smoothPlayers, type SmoothPlayer } from "../src/viewer/sync";
import { MOCK_BIPEDS, encodeModelGlb } from "../mock/models";

const fp = (over: Partial<FramePlayer> = {}): FramePlayer => ({ index: 0, alive: true, pos: [0, 0, 0], yaw: 0, ...over });
const frame = (tick: number, players: FramePlayer[], extra: Partial<Frame> = {}): Frame => ({ game_id: "g", tick, players, ...extra });

// ---- a fixture stream: a player running over a hill, recorded once a second ----

/** The level: a hill across x, its crest at x = 6. */
const terrain = (x: number) => 1.5 * Math.exp(-((x - 6) ** 2) / 8);
const slope = (x: number) => terrain(x) * (-(2 * (x - 6)) / 8);
/** Where the game had the runner at `tick`: 3 units a second along x, feet on the ground. */
const runner = (tick: number) => { const x = (tick / 30) * 3; return { x, z: terrain(x), vx: 3, vz: 3 * slope(x) }; };
/** One frame a second (as the server records), with the game's stance and velocity. */
const recorded: Frame[] = [0, 30, 60, 90, 120].map((tick) => {
  const r = runner(tick);
  return frame(tick, [fp({ pos: [r.x, 0, r.z], stance: "stand", speed: 3, vel: [r.vx, 0, r.vz], anim: { name: "combat:rifle:any:move_front", frame: (tick % 24) } })]);
});
/** The level's surface under a point, as the scene's probe finds it. */
const probe = (x: number, _y: number, z: number, reach: number): Floor | null => {
  const h = terrain(x);
  if (h > z || h < z - reach) return null;
  const n = new THREE.Vector3(-slope(x), 0, 1).normalize();
  return { x, y: 0, z: h, nx: n.x, ny: n.y, nz: n.z };
};

describe("a recorded runner over a hill", () => {
  test("between samples a straight line leaves the ground: the float the viewer showed", () => {
    // Without velocities the sampler draws a straight line between samples a second apart.
    const flat = recorded.map((f) => ({ ...f, players: f.players.map((p) => ({ ...p, vel: null })) }));
    let worst = 0;
    for (let t = 0; t <= 120; t += 1) {
      const s = sampleFrames(flat, t)[0]!;
      worst = Math.max(worst, Math.abs(s.z - terrain(s.x)));
    }
    expect(worst).toBeGreaterThan(0.15);
  });

  test("with the game's velocities the path bends with the hill, and the feet go on the floor exactly", () => {
    const floors = new GroundCache(probe);
    const shown = new Map<number, SmoothPlayer>();
    let curved = 0, onFloor = 0;
    for (let t = 0; t <= 120; t += 0.5) {
      floors.begin();
      const sampled = sampleFrames(recorded, t);
      smoothPlayers(shown, sampled, 16);
      const s = shown.get(0)!;
      // Taken as sampled: no easing lag behind a sample with a velocity.
      expect(s.x).toBe(sampled[0]!.x);
      curved = Math.max(curved, Math.abs(s.z - terrain(s.x)));
      const feet = floors.place(0, s);
      onFloor = Math.max(onFloor, Math.abs(feet - terrain(s.x)));
    }
    // Hermite is much closer than the straight line already, and the floor puts the feet on it.
    expect(curved).toBeLessThan(0.15);
    expect(onFloor).toBeLessThan(0.02);
  });

  test("the run animation's frame runs on between samples at the game's rate", () => {
    const s = sampleFrames(recorded, 40)[0]!;
    expect(s.anim?.name).toBe("combat:rifle:any:move_front");
    // 30 % 24 = 6 at tick 30, 60 % 24 = 12 at tick 60: a wrapped loop, so the frame is carried on from the nearer.
    expect(s.anim!.frame).toBeCloseTo(6 + 10, 6);
  });
});

describe("hermite sampling", () => {
  test("meets both samples and leaves them at their velocities", () => {
    expect(hermite(1, 5, 4, -2, 0)).toBe(1);
    expect(hermite(1, 5, 4, -2, 1)).toBe(4);
    const d = 1e-6;
    expect((hermite(1, 5, 4, -2, d) - 1) / d).toBeCloseTo(5, 3);
    expect((4 - hermite(1, 5, 4, -2, 1 - d)) / d).toBeCloseTo(-2, 3);
  });

  test("a turn between two samples is a curve, a straight run stays straight", () => {
    // Heading +x, then +y a second later: the curve swings out past the chord.
    const fs = [frame(0, [fp({ pos: [0, 0, 0], vel: [3, 0, 0] })]), frame(30, [fp({ pos: [2, 2, 0], vel: [0, 3, 0] })])];
    const mid = sampleFrames(fs, 15)[0]!;
    expect(mid.x).toBeGreaterThan(1.2);
    expect(mid.y).toBeLessThan(0.8);
    const line = [frame(0, [fp({ pos: [0, 0, 0], vel: [3, 0, 0] })]), frame(30, [fp({ pos: [3, 0, 0], vel: [3, 0, 0] })])];
    expect(sampleFrames(line, 10)[0]!.x).toBeCloseTo(1, 9);
    expect(sampleFrames(line, 10)[0]!.vel).toEqual([3, 0, 0]);
  });

  test("samples too far apart, or without a velocity on both sides, are a straight line", () => {
    const far = [frame(0, [fp({ pos: [0, 0, 0], vel: [3, 0, 0] })]), frame(HERMITE_MAX_TICKS + 2, [fp({ pos: [2, 2, 0], vel: [0, 3, 0] })])];
    const s = sampleFrames(far, (HERMITE_MAX_TICKS + 2) / 2)[0]!;
    expect(s.x).toBeCloseTo(1, 9);
    expect(s.y).toBeCloseTo(1, 9);
    const one = [frame(0, [fp({ pos: [0, 0, 0], vel: [3, 0, 0] })]), frame(30, [fp({ pos: [2, 2, 0] })])];
    expect(sampleFrames(one, 15)[0]!.x).toBeCloseTo(1, 9);
  });

  test("actors are sampled the same way, with their animation and seat", () => {
    const actor = (tick: number, x: number) => frame(tick, [], { actors: [{ id: "a1", species: "grunt", pos: [x, 0, 0], yaw: 0, vel: [3, 0, 0], anim: { name: "combat:pistol:any:idle", frame: tick }, vehicle: "v1", seat: 2 }] });
    const s = sampleActors([actor(0, 0), actor(30, 3)], 10)[0]!;
    expect(s.x).toBeCloseTo(1, 9);
    expect(s.anim).toEqual({ name: "combat:pistol:any:idle", frame: 10 });
    expect(s.seat).toBe(2);
  });

  test("the contract's checks", () => {
    expect(cleanVel([1, 2, 3])).toEqual([1, 2, 3]);
    expect(cleanVel([1, 2])).toBeNull();
    expect(cleanVel([1000, 0, 0])).toBeNull();
    expect(cleanVel([NaN, 0, 0])).toBeNull();
    expect(cleanAnim({ name: "combat:rifle:any:idle", frame: 3.5 })).toEqual({ name: "combat:rifle:any:idle", frame: 3.5 });
    expect(cleanAnim({ name: "<script>", frame: 1 })).toBeNull();
    expect(cleanAnim({ name: "combat:rifle:any:idle", frame: -1 })).toBeNull();
    expect(cleanSeat(3)).toBe(3);
    expect(cleanSeat(-1)).toBeNull();
    expect(cleanSeat(1.5)).toBeNull();
    // A seat without a vehicle means nothing.
    expect(sampleFrames([frame(0, [fp({ seat: 1 })])], 0)[0]!.seat).toBeUndefined();
    expect(sampleFrames([frame(0, [fp({ seat: 1, vehicle: "v" })])], 0)[0]!.seat).toBe(1);
  });
});

describe("the game's animation between samples", () => {
  test("one animation on both sides moves between their frames", () => {
    expect(blendAnim({ name: "a", frame: 10 }, { name: "a", frame: 40 }, 0, 30, 15)).toEqual({ name: "a", frame: 25 });
    expect(blendAnim({ name: "a", frame: 10 }, { name: "a", frame: 10 }, 0, 30, 15)).toEqual({ name: "a", frame: 10 });
    // A loop that wrapped between them: carried on from the nearer.
    expect(blendAnim({ name: "a", frame: 10 }, { name: "a", frame: 12 }, 0, 30, 5)).toEqual({ name: "a", frame: 15 });
  });
  test("a change is the nearer sample's, carried on a frame a tick", () => {
    expect(blendAnim({ name: "a", frame: 10 }, { name: "b", frame: 4 }, 0, 30, 10)).toEqual({ name: "a", frame: 20 });
    expect(blendAnim({ name: "a", frame: 10 }, { name: "b", frame: 4 }, 0, 30, 28)).toEqual({ name: "b", frame: 2 });
    // Never before its start.
    expect(blendAnim({ name: "a", frame: 10 }, { name: "b", frame: 4 }, 0, 30, 20)).toEqual({ name: "b", frame: 0 });
    expect(blendAnim(null, null, 0, 30, 10)).toBeNull();
  });
});

describe("the game's animation names to clips", () => {
  test("locomotion, crouching, strafing and backing up", () => {
    expect(gameAnim("combat:rifle:any:idle", false)).toMatchObject({ state: "idle", cls: "rifle", loop: true });
    expect(gameAnim("combat:rifle:any:move_front", false)).toMatchObject({ state: "run", loop: true });
    expect(gameAnim("combat:pistol:any:walk_front", false)).toMatchObject({ state: "walk", cls: "pistol" });
    expect(gameAnim("combat:rifle:any:move_left", false)?.state).toBe("run_left");
    expect(gameAnim("combat:rifle:any:move_back", false)?.state).toBe("run_back");
    expect(gameAnim("crouch:rifle:any:idle", false)?.state).toBe("crouch");
    expect(gameAnim("crouch:rifle:any:move_front", false)?.state).toBe("crouch_run");
    expect(gameAnim("crouch:rifle:any:move_right", false)?.state).toBe("crouch_right");
    expect(sideways("run_left")).toBe(true);
    expect(sideways("run")).toBe(false);
  });

  test("one-shots and deaths are held, not looped; a class of `any` leaves the weapon's", () => {
    expect(gameAnim("combat:rifle:any:airborne", false)).toMatchObject({ state: "airborne", loop: false });
    expect(gameAnim("combat:rifle:any:land_hard", false)?.state).toBe("land_hard");
    expect(gameAnim("combat:rifle:br:reload_1", false)).toMatchObject({ state: "reload", cls: "rifle", loop: false });
    expect(gameAnim("combat:rifle:any:throw_grenade", false)?.state).toBe("grenade");
    expect(gameAnim("combat:rifle:any:melee_strike_1:var1", false)?.state).toBe("melee");
    expect(gameAnim("combat:any:any:s_kill:front:gut", false)).toMatchObject({ state: "die", cls: null });
    expect(gameAnim("combat:any:any:landing_dead", false)?.state).toBe("dead");
    expect(gameAnim("combat:rifle:any:something_new", false)).toBeNull();
    expect(gameAnim("idle", false)).toBeNull();
  });

  test("a rider sits in their seat's mode", () => {
    expect(gameAnim("warthog_d:any:any:idle", true)).toMatchObject({ state: "seat_warthog_d", mode: "warthog_d", loop: true });
    expect(gameAnim("warthog_p:rifle:any:idle", true)).toMatchObject({ state: "seat_warthog_p", cls: "rifle" });
  });

  test("clips the glb has, and the fall-backs for art without the new ones", () => {
    const v22 = ["rifle:idle", "rifle:run", "rifle:run_left", "rifle:crouch", "rifle:crouch_run", "any:seat_warthog_d"];
    expect(clipFor(v22, "rifle", "run_left")).toBe("rifle:run_left");
    expect(clipFor(v22, "rifle", "seat_warthog_d")).toBe("any:seat_warthog_d");
    const v21 = ["rifle:idle", "rifle:walk", "rifle:run", "rifle:crouch"];
    expect(clipFor(v21, "rifle", "run_left")).toBe("rifle:run");
    expect(clipFor(v21, "rifle", "walk_back")).toBe("rifle:walk");
    expect(clipFor(v21, "rifle", "crouch_left")).toBe("rifle:crouch");
    expect(clipFor(v21, "rifle", "seat_ghost_d")).toBe("rifle:idle");
  });

  test("the game's frame is the clip's time: 30 a second, wrapped when it loops, held when not", () => {
    // A 24-frame loop: frames 0..23, 23/30 s long.
    const d = 23 / 30;
    expect(clipTime(12, d, true)).toBeCloseTo(0.4, 9);
    expect(clipTime(24, d, true)).toBeCloseTo(0, 9);
    expect(clipTime(30, d, true)).toBeCloseTo(0.2, 9);
    expect(clipTime(40, d, false)).toBeCloseTo(d, 9);
    expect(clipTime(5, 0, true)).toBe(0);
  });
});

describe("feet on the floor", () => {
  const s = (over: Partial<SmoothPlayer> = {}) => ({ x: 0, y: 0, z: 1, alive: true, stance: "stand" as const, vehicle: null, ...over });

  test("only a biped the game has on its feet is put on the floor", () => {
    expect(onFoot(s())).toBe(true);
    expect(onFoot(s({ stance: "crouch" }))).toBe(true);
    expect(onFoot(s({ stance: "airborne" }))).toBe(false);
    expect(onFoot(s({ stance: null }))).toBe(false);
    expect(onFoot(s({ vehicle: "v" }))).toBe(false);
    expect(onFoot(s({ alive: false }))).toBe(false);
  });

  test("a floor near the sampled height takes the feet; one far away doesn't", () => {
    expect(footZ(1, 0.8)).toBe(0.8);
    expect(footZ(1, 1 + GROUND_UP - 0.01)).toBeCloseTo(1 + GROUND_UP - 0.01, 9);
    expect(footZ(1, 1 + GROUND_UP + 0.01)).toBe(1);
    expect(footZ(1, 1 - GROUND_DOWN - 0.01)).toBe(1);
    expect(footZ(1, null)).toBe(1);
  });

  test("the floor is probed again once a biped moves, its plane used in between, a few probes an update", () => {
    let probes = 0;
    const flat = new GroundCache((x, y, z, reach) => { probes++; return probe(x, y, z, reach); });
    flat.begin();
    expect(flat.place(0, s({ x: 6, z: terrain(6) + 0.2 }))).toBeCloseTo(terrain(6), 9);
    expect(flat.place(0, s({ x: 6.01, z: terrain(6) + 0.2 }))).toBeCloseTo(terrain(6.01), 4);
    expect(probes).toBe(1);
    expect(flat.place(0, s({ x: 7, z: terrain(7) }))).toBeCloseTo(terrain(7), 9);
    expect(probes).toBe(2);
    // In the air: where the frames have them, not probed.
    expect(flat.place(0, s({ x: 8, z: 3, stance: "airborne" }))).toBe(3);
    expect(probes).toBe(2);
    flat.begin();
    for (let i = 0; i < GROUND_BUDGET + 5; i++) flat.place(100 + i, s({ x: i, z: terrain(i) }));
    expect(probes).toBe(2 + GROUND_BUDGET);
  });

  test("a plane's height off its point", () => {
    const n = new THREE.Vector3(-0.5, 0, 1).normalize();
    expect(floorAt({ x: 0, y: 0, z: 1, nx: n.x, ny: n.y, nz: n.z }, 2, 5)).toBeCloseTo(2, 9);
    // A wall: its own point's height.
    expect(floorAt({ x: 0, y: 0, z: 1, nx: 1, ny: 0, nz: 0 }, 2, 5)).toBe(1);
  });
});

describe("riders in their seats", () => {
  const nodes = ["seat_driver", "seat_gunner", "seat_passenger"];

  test("the seat table's node for their seat, else their mode's", () => {
    const table = [{ label: "warthog_d", node: "seat_driver" }, { label: "warthog_p", node: "seat_passenger" }, { label: "warthog_g", node: "seat_gunner" }];
    expect(seatNodeFor(nodes, { seat: 2 }, table)).toBe("seat_gunner");
    expect(seatNodeFor(nodes, { mode: "warthog_p" }, table)).toBe("seat_passenger");
  });

  test("without a table, the role their mode ends in", () => {
    expect(seatNodeFor(nodes, { mode: "warthog_d" })).toBe("seat_driver");
    expect(seatNodeFor(nodes, { mode: "warthog_g" })).toBe("seat_gunner");
    expect(seatNodeFor(nodes, { mode: "warthog_p" })).toBe("seat_passenger");
    expect(seatNodeFor(["seat_passenger_lf", "seat_passenger_rf"], { mode: "scorpion_p_rf" })).toBe("seat_passenger_rf");
    expect(seatNodeFor(["seat_passenger_lf"], { mode: "scorpion_p_rb" })).toBe("seat_passenger_lf");
    expect(seatNodeFor(nodes, { mode: "flying" })).toBeNull();
    expect(seatNodeFor([], { mode: "ghost_d" })).toBeNull();
  });

  test("an older sender: a turret's rider is its gunner, the first rider the driver, others nowhere", () => {
    expect(seatNodeFor(nodes, { turret: true })).toBe("seat_gunner");
    expect(seatNodeFor(nodes, { first: true })).toBe("seat_driver");
    expect(seatNodeFor(nodes, {})).toBeNull();
    expect(seatState("warthog", "seat_driver")).toBe("seat_warthog_d");
    expect(seatState("warthog", "seat_gunner")).toBe("seat_warthog_g");
  });

  test("the models table's seats are read and checked", () => {
    const m = parseModels({
      version: 2, bipeds: { spartan: { glb: "b.glb" } }, vehicles: { warthog: { glb: "w.glb" } },
      seats: { warthog: [{ label: "warthog_d", node: "seat_driver" }, { label: "bad label!", node: "elsewhere" }], "Bad Id": [] },
    })!;
    expect(m.seats).toEqual({ warthog: [{ label: "warthog_d", node: "seat_driver" }, { label: null, node: null }] });
    expect(parseModels({ version: 2, bipeds: { spartan: { glb: "b.glb" } } })!.seats).toBeUndefined();
  });
});

describe("a model where the game draws it", () => {
  const loaded = (extras: Record<string, unknown>) => {
    const scene = new THREE.Group();
    const mesh = new THREE.Mesh(new THREE.BoxGeometry(0.4, 0.7, 0.4));
    mesh.position.y = 0.35;
    scene.add(mesh);
    return { scene, asset: { extras } };
  };

  test("a biped the extractor raised onto its lowest vertex goes back down by as much", () => {
    const t = prepareBiped(loaded({ height: 0.7, lift: 0.05 }))!;
    const copy = t.root.clone(true);
    copy.updateWorldMatrix(true, true);
    expect(new THREE.Box3().setFromObject(copy).min.y).toBeCloseTo(-0.05, 6);
    expect(t.height).toBe(0.7);
  });

  test("art before extractor 22 says nothing, and stays where it stands", () => {
    expect(prepareBiped(loaded({ height: 0.7 }))!.root.position.y).toBe(0);
    expect(prepareBiped(loaded({ height: 0.7, lift: 40 }))!.root.position.y).toBe(0);
  });
});

describe("the game's animation on a model", () => {
  const parse = (bytes: Uint8Array) => new GLTFLoader().parseAsync(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer, "");

  test("a clip put at the game's frame stays there, and doesn't run on by itself", async () => {
    const bytes = encodeModelGlb(MOCK_BIPEDS.spartan!(true));
    const lib = new ModelLibrary(() => {}, () => parse(bytes));
    lib.biped("a");
    await new Promise((r) => setTimeout(r, 20));
    const t = lib.biped("a")!;
    const a = lib.instantiateBiped(t, "#3a7bd5"), b = lib.instantiateBiped(t, "#3a7bd5");
    const thigh = (o: THREE.Object3D) => { let q: THREE.Quaternion | null = null; o.traverse((n) => { if (!q && /thigh_l/.test(n.name) && !(n as THREE.Mesh).isMesh) q = n.quaternion.clone(); }); return q!; };
    // The same frame of the run on two copies, one reached by updates and one put there.
    a.animator!.setLoopAt("rifle:run", clipTime(9, a.animator!.duration("rifle:run")! / 1000, true));
    a.animator!.update(16);
    const at = thigh(a.object);
    a.animator!.update(200);
    expect(thigh(a.object).angleTo(at)).toBeLessThan(1e-6);
    b.animator!.setLoop("rifle:run");
    b.animator!.update(0);
    b.animator!.update(300);
    expect(thigh(b.object).angleTo(at)).toBeLessThan(1e-3);
  });
});

// Keep the sampled player shape in step with what the scene reads.
const _shape: Pick<SampledPlayer, "vel" | "anim" | "seat"> = { vel: [0, 0, 0], anim: { name: "a", frame: 0 }, seat: 0 };
void _shape;
