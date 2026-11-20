// What a shot looks like in the 3D view, by what fired it: a fast gun's
// tracer streak, a plasma bolt or a needle that visibly travels, a rocket
// with its trail, a sniper's beam, and for a sword or a melee no shot at
// all (the model swings instead). Speeds come from the art manifest's
// per-weapon ballistics, read from the weapon's projectile tag on the
// owner's disc (docs/live/live-art.md); the table here only fills gaps.
//
// Each shot flies from the weapon's muzzle along the aim and ends where
// the frame says it hit someone, else at the first piece of level it
// meets, else at the kind's range.

import * as THREE from "three";

export type ShotKind = "bullet" | "plasma" | "needle" | "rocket" | "grenade" | "beam" | "melee";
const KINDS: readonly ShotKind[] = ["bullet", "plasma", "needle", "rocket", "grenade", "beam", "melee"];

/** What the manifest says about a weapon's shots (any part may be missing). */
export interface DiscBallistics {
  kind?: string;
  /** World units a second as it leaves the barrel, and at the end of its acceleration. */
  speed?: number;
  final_speed?: number;
  /** Rounds a second. */
  rate?: number;
}

export interface Look {
  kind: ShotKind;
  /** World units a second at the muzzle, and the speed it gains to. */
  speed: number;
  finalSpeed: number;
  /** The glow's colour. */
  colour: number;
  /** A tracer's or trail's length, and a bolt's radius (world units). */
  length: number;
  size: number;
  /** How far it goes when it meets nothing. */
  range: number;
}

/** By kind: what a weapon's shots look like when nothing more is known. */
export const KIND_LOOKS: Record<ShotKind, Look> = {
  bullet: { kind: "bullet", speed: 400, finalSpeed: 400, colour: 0xffd27a, length: 2.2, size: 0.006, range: 60 },
  plasma: { kind: "plasma", speed: 14, finalSpeed: 14, colour: 0x4f9cff, length: 0.45, size: 0.03, range: 40 },
  needle: { kind: "needle", speed: 6, finalSpeed: 6, colour: 0xff4fd0, length: 0.25, size: 0.014, range: 35 },
  rocket: { kind: "rocket", speed: 8, finalSpeed: 20, colour: 0xff8a2a, length: 1.4, size: 0.04, range: 60 },
  grenade: { kind: "grenade", speed: 20, finalSpeed: 10, colour: 0xff7a26, length: 0.7, size: 0.035, range: 40 },
  beam: { kind: "beam", speed: Infinity, finalSpeed: Infinity, colour: 0xff5cff, length: 0, size: 0.008, range: 100 },
  melee: { kind: "melee", speed: 0, finalSpeed: 0, colour: 0xffffff, length: 0, size: 0, range: 0 },
};

/** Weapons whose shots look unlike their kind's default, by weapon stem. */
const WEAPON_LOOKS: Record<string, Partial<Look>> = {
  plasma_pistol: { kind: "plasma", colour: 0x5cff5a },
  plasma_rifle: { kind: "plasma", colour: 0x4f9cff },
  brute_plasma_rifle: { kind: "plasma", colour: 0xff5a30 },
  needler: { kind: "needle" },
  rocket_launcher: { kind: "rocket" },
  flak_cannon: { kind: "plasma", colour: 0x7dff2a, size: 0.06, length: 0.8 },
  brute_shot: { kind: "grenade" },
  beam_rifle: { kind: "beam", colour: 0xff7cff },
  sniper_rifle: { kind: "beam", colour: 0xfff0c8, size: 0.006 },
  covenant_carbine: { kind: "bullet", colour: 0x6aff4a },
  energy_blade: { kind: "melee" },
  energy_blade_useless: { kind: "melee" },
};

/** How a weapon's shots look: the weapon's own colours and kind, the disc's
 *  speeds and kind when the manifest has them, the kind's defaults for the
 *  rest. A sword class (or an unknown weapon with no class) melees or
 *  shoots bullets. */
export function lookFor(weapon: string | null | undefined, cls: string | null | undefined, disc?: DiscBallistics | null): Look {
  const own = (weapon && WEAPON_LOOKS[weapon]) || {};
  const discKind = disc?.kind && (KINDS as readonly string[]).includes(disc.kind) ? (disc.kind as ShotKind) : undefined;
  const kind: ShotKind = cls === "sword" ? "melee" : own.kind ?? discKind ?? "bullet";
  const look: Look = { ...KIND_LOOKS[kind], ...own, kind };
  const ok = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v > 0;
  if (kind !== "beam" && kind !== "melee" && ok(disc?.speed)) {
    look.speed = disc.speed;
    look.finalSpeed = ok(disc.final_speed) ? disc.final_speed : disc.speed;
  }
  return look;
}

/** How far a shot has flown `t` seconds after it left: steady, or (a
 *  rocket) speeding up from `speed` to `finalSpeed` over its first second. */
export function flown(look: Pick<Look, "speed" | "finalSpeed">, t: number): number {
  if (!Number.isFinite(look.speed)) return Infinity;
  const ramp = 1, a = (look.finalSpeed - look.speed) / ramp;
  if (t <= ramp) return look.speed * t + 0.5 * a * t * t;
  return look.speed * ramp + 0.5 * a * ramp * ramp + look.finalSpeed * (t - ramp);
}

/** Where a shot from `from` along `dir` ends, and how far that is: where the
 *  frame says it hit someone (if that is roughly ahead), else what `cast`
 *  finds first, else the look's range. */
export function shotEnd(
  from: THREE.Vector3, dir: THREE.Vector3, range: number,
  hit: THREE.Vector3 | null, cast: ((from: THREE.Vector3, dir: THREE.Vector3, far: number) => number | null) | null,
): number {
  if (hit) {
    const to = hit.clone().sub(from);
    const d = to.length();
    // A hit behind the shooter, or far off the line, is someone else's shot.
    if (d > 0 && d <= range * 1.5 && to.dot(dir) / d > 0.8) return d;
  }
  const c = cast?.(from, dir, range);
  return c !== null && c !== undefined && c > 0 ? Math.min(c, range) : range;
}

interface Flying {
  look: Look;
  from: THREE.Vector3;
  dir: THREE.Vector3;
  dist: number;
  born: number;
  object: THREE.Mesh;
  trail: THREE.Mesh | null;
}

/** How long a beam stays, and how long a bolt's impact glow lasts (ms). */
export const BEAM_MS = 320;
/** The longest a shot is drawn for, however slow (ms). */
export const MAX_FLIGHT_MS = 4000;

/** The shots in flight, drawn into a scene. */
export class Projectiles {
  private live: Flying[] = [];
  private spare = new Map<string, THREE.Mesh[]>();
  private geoBeam = new THREE.CylinderGeometry(1, 1, 1, 6, 1, true).rotateZ(-Math.PI / 2).translate(-0.5, 0, 0);
  private geoBolt = new THREE.SphereGeometry(1, 10, 8);
  private geoBody = new THREE.CylinderGeometry(0.5, 0.5, 1, 8).rotateZ(-Math.PI / 2);
  private q = new THREE.Quaternion();

  constructor(private scene: THREE.Object3D) {}

  get count(): number { return this.live.length; }

  private mesh(key: string, make: () => THREE.Mesh): THREE.Mesh {
    const m = this.spare.get(key)?.pop() ?? make();
    m.userData.key = key;
    m.visible = true;
    if (!m.parent) this.scene.add(m);
    return m;
  }

  private glow(colour: number): THREE.MeshBasicMaterial {
    // Not tone mapped: a bolt keeps its colour rather than washing to white.
    return new THREE.MeshBasicMaterial({ color: colour, transparent: true, depthWrite: false, fog: false, toneMapped: false, blending: THREE.AdditiveBlending, side: THREE.DoubleSide });
  }

  /** Starts a shot: from the muzzle (scene coordinates) along `dir` (unit), `dist` to its end. */
  fire(look: Look, from: THREE.Vector3, dir: THREE.Vector3, dist: number, now: number): void {
    if (look.kind === "melee" || dist <= 0) return;
    const glow = (key: string, geo: THREE.BufferGeometry, colour: number) =>
      this.mesh(key + colour, () => new THREE.Mesh(geo, this.glow(colour)));
    let object: THREE.Mesh, trail: THREE.Mesh | null = null;
    switch (look.kind) {
      case "beam":
      case "bullet":
        object = glow("beam", this.geoBeam, look.colour);
        break;
      case "rocket":
      case "grenade":
        object = this.mesh("body", () => new THREE.Mesh(this.geoBody, new THREE.MeshStandardMaterial({ color: 0x3b3f44, roughness: 0.5, metalness: 0.6 })));
        trail = glow("beam", this.geoBeam, look.colour);
        break;
      default:
        object = glow("bolt", this.geoBolt, look.colour);
        trail = glow("beam", this.geoBeam, look.colour);
    }
    this.q.setFromUnitVectors(new THREE.Vector3(1, 0, 0), dir);
    for (const m of [object, trail]) if (m) { m.quaternion.copy(this.q); m.renderOrder = 4; }
    this.live.push({ look, from: from.clone(), dir: dir.clone(), dist, born: now, object, trail });
  }

  /** Moves every shot to `now`; true while any is in flight. */
  update(now: number, visible?: (at: THREE.Vector3) => boolean): boolean {
    const keep: Flying[] = [];
    for (const f of this.live) {
      const t = (now - f.born) / 1000;
      const done = this.place(f, t);
      if (done || now - f.born > MAX_FLIGHT_MS) { this.release(f.object); if (f.trail) this.release(f.trail); }
      else {
        f.object.visible = visible?.(f.object.position) ?? true;
        if (f.trail) f.trail.visible = f.object.visible;
        keep.push(f);
      }
    }
    this.live = keep;
    return keep.length > 0;
  }

  /** Puts a shot where it is `t` seconds in; true when it has finished. */
  private place(f: Flying, t: number): boolean {
    const { look, object, trail } = f;
    const mat = (m: THREE.Mesh) => m.material as THREE.MeshBasicMaterial;
    if (look.kind === "beam") {
      // The whole line at once, fading.
      const k = 1 - (t * 1000) / BEAM_MS;
      if (k <= 0) return true;
      object.position.copy(f.from).addScaledVector(f.dir, f.dist);
      object.scale.set(f.dist, look.size, look.size);
      mat(object).opacity = 0.9 * k;
      return false;
    }
    const went = flown(look, t);
    const head = Math.min(f.dist, went);
    const arrived = went >= f.dist;
    // A tracer's tail follows its head at the same speed, and catches it at the end.
    const tail = Math.min(head, Math.max(0, went - look.length));
    if (arrived && tail >= f.dist - 1e-3) return true;
    const at = new THREE.Vector3().copy(f.from).addScaledVector(f.dir, head);
    if (look.kind === "bullet") {
      object.position.copy(at);
      object.scale.set(Math.max(1e-3, head - tail), look.size, look.size);
      mat(object).opacity = 0.85;
      return false;
    }
    if (arrived) return true;
    object.position.copy(at);
    if (look.kind === "rocket" || look.kind === "grenade") object.scale.set(look.size * 4, look.size, look.size);
    else object.scale.setScalar(look.size);
    if (trail) {
      trail.position.copy(at);
      trail.scale.set(Math.max(1e-3, Math.min(head, look.length)), look.size * 0.45, look.size * 0.45);
      mat(trail).opacity = 0.6;
    }
    return false;
  }

  private release(m: THREE.Mesh): void {
    m.visible = false;
    const key = m.userData.key as string;
    let list = this.spare.get(key);
    if (!list) { list = []; this.spare.set(key, list); }
    list.push(m);
  }

  dispose(): void {
    for (const f of this.live) { this.release(f.object); if (f.trail) this.release(f.trail); }
    this.live = [];
    for (const list of this.spare.values()) for (const m of list) { m.removeFromParent(); (m.material as THREE.Material).dispose(); }
    this.spare.clear();
    this.geoBeam.dispose(); this.geoBolt.dispose(); this.geoBody.dispose();
  }
}
