// Direction ticks, focused damage lines, kill confirms and recorded
// projectiles in the 3D view. The clock supplies each projectile
// path, and pools keep geometry and materials reusable.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import type { VisibleFilmEffect } from "./film-world";
import { gameToGl } from "../map/views";
import { entityIndex, hitColour, type LiveHit } from "../map/events";
import { MAX_TRAIL_POINTS, projectileColour, type ProjectileTrail } from "../state/projectile-trails";

interface HitObj {
  group: THREE.Group;
  beam: THREE.Mesh<THREE.BufferGeometry, THREE.MeshBasicMaterial>;
  tick: THREE.Mesh<THREE.BufferGeometry, THREE.MeshBasicMaterial>;
  confirm: THREE.Sprite;
}
interface TrailObj { group: THREE.Group; ball: THREE.Mesh; mat: THREE.MeshBasicMaterial; trail: THREE.Line; trailMat: THREE.LineBasicMaterial }

const AXIS_X = new THREE.Vector3(1, 0, 0);
/** A hit lands about chest height on whoever it names (game units above their feet). */
export const CHEST = 0.45;

/** The kill confirm: a ring around four ticks. */
function confirmTexture(): THREE.Texture {
  const c = document.createElement("canvas");
  c.width = c.height = 64;
  const ctx = c.getContext("2d")!;
  ctx.lineCap = "round";
  for (const [style, w] of [["rgba(0,0,0,.85)", 9], ["#fff", 5]] as const) {
    ctx.strokeStyle = style; ctx.lineWidth = w;
    ctx.beginPath();
    for (const [dx, dy] of [[1, 1], [1, -1], [-1, 1], [-1, -1]] as const) { ctx.moveTo(32 + dx * 9, 32 + dy * 9); ctx.lineTo(32 + dx * 19, 32 + dy * 19); }
    ctx.moveTo(32 + 26, 32); ctx.arc(32, 32, 26, 0, Math.PI * 2);
    ctx.stroke();
  }
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

export class Effects3D {
  private sparks: THREE.Mesh<THREE.SphereGeometry, THREE.MeshBasicMaterial>[] = [];
  private hits = new Map<number, HitObj>();
  private spareHits: HitObj[] = [];
  private trails = new Map<string, TrailObj>();
  private spareTrails: TrailObj[] = [];
  private geoBeam = new THREE.CylinderGeometry(1, 1, 1, 5, 1, true).rotateZ(-Math.PI / 2).translate(0.5, 0, 0);
  private geoBall = new THREE.SphereGeometry(1, 10, 8);
  private geoTick = new THREE.RingGeometry(0.34, 0.46, 12, 1, -Math.PI / 6, Math.PI / 3).rotateX(-Math.PI / 2);
  private texKill: THREE.Texture | null = null;
  private a = new THREE.Vector3();
  private b = new THREE.Vector3();

  constructor(private scene: THREE.Scene, private camera: THREE.Camera) {}

  /** Damage-report origins and the final observed segment of a projectile.
   * Deletion alone draws no explosion. Age is measured on the film clock,
   * so these effects freeze, rewind and resume with the simulation. */
  film(events: readonly VisibleFilmEffect[], still: boolean, visible?: (at: THREE.Vector3) => boolean) {
    let used = 0, changed = false;
    for (const e of events) {
      if (e.kind === "barrel") continue;
      let mark = this.sparks[used++];
      if (!mark) {
        const mat = new THREE.MeshBasicMaterial({ transparent: true, depthWrite: false, blending: THREE.AdditiveBlending });
        mark = new THREE.Mesh(this.geoBall, mat);
        mark.renderOrder = 3;
        this.scene.add(mark);
        this.sparks.push(mark);
      }
      const blast = e.kind === "blast";
      const terminal = e.kind === "terminal";
      const life = e.age / (blast ? 12 : 4);
      const r = blast ? .14 + (still ? .13 : life * .45) : terminal ? .017 : .04 + life * .06;
      mark.position.set(...gameToGl(...e.pos));
      mark.scale.setScalar(r);
      mark.quaternion.identity();
      if (terminal && e.direction) {
        // Show the direction in which it arrived. Clamp the visual trail
        // length so a high-speed shot cannot span the entire level.
        this.a.set(...gameToGl(...e.direction));
        const speed = this.a.length();
        if (speed > 1e-6) {
          const half = Math.min(.65, speed / 60);
          this.a.divideScalar(speed);
          mark.quaternion.setFromUnitVectors(AXIS_X, this.a);
          mark.scale.x = Math.max(r, half);
          mark.position.addScaledVector(this.a, -half);
        }
      }
      mark.visible = visible?.(mark.position) ?? true;
      mark.material.color.setHex(blast ? 0xff932f : terminal ? 0xffe0aa : 0xffc76b);
      mark.material.opacity = (1 - life) * (still ? .45 : .82);
      changed = true;
    }
    for (let i = used; i < this.sparks.length; i++) {
      const spark = this.sparks[i]!;
      if (!spark.visible) continue;
      spark.visible = false;
      changed = true;
    }
    return changed;
  }

  private hit(): HitObj {
    const old = this.spareHits.pop();
    if (old) return old;
    const beam = new THREE.Mesh(this.geoBeam, new THREE.MeshBasicMaterial({
      transparent: true, depthWrite: false, fog: false, blending: THREE.AdditiveBlending, side: THREE.DoubleSide,
    }));
    const tick = new THREE.Mesh(this.geoTick, new THREE.MeshBasicMaterial({ transparent: true, depthWrite: false, fog: false }));
    const confirm = new THREE.Sprite(new THREE.SpriteMaterial({
      transparent: true, depthTest: false, depthWrite: false,
      fog: false, sizeAttenuation: false, toneMapped: false,
    }));
    beam.renderOrder = tick.renderOrder = 4;
    confirm.renderOrder = 6;
    const group = new THREE.Group();
    group.add(beam, tick, confirm);
    this.scene.add(group);
    return { group, beam, tick, confirm };
  }

  private trail(): TrailObj {
    const old = this.spareTrails.pop();
    if (old) { old.group.visible = true; return old; }
    const mat = new THREE.MeshBasicMaterial({ transparent: true, fog: false });
    const ball = new THREE.Mesh(this.geoBall, mat);
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(MAX_TRAIL_POINTS * 3), 3));
    const trailMat = new THREE.LineBasicMaterial({ transparent: true, depthWrite: false, fog: false });
    const trail = new THREE.Line(geo, trailMat);
    trail.frustumCulled = false;
    ball.renderOrder = trail.renderOrder = 4;
    const group = new THREE.Group();
    group.add(ball, trail);
    this.scene.add(group);
    return { group, ball, mat, trail, trailMat };
  }

  /** Draws damage and recorded projectiles, sharing pools across frames. */
  update(
    hits: readonly LiveHit[], trails: readonly ProjectileTrail[], scale: number,
    view: {
      visible: (at: THREE.Vector3) => boolean;
      focused: (index: number) => boolean;
      colour: (index: number) => string | undefined;
      ring: (index: number) => { position: THREE.Vector3; scale: number } | undefined;
    },
  ): boolean {
    let changed = hits.length > 0 || trails.length > 0;
    const kept = new Set<number>();
    for (const h of hits) {
      if (!h.from && !h.kill) continue;
      let o = this.hits.get(h.id);
      if (!o) { o = this.hit(); this.hits.set(h.id, o); }
      kept.add(h.id);
      const fromIndex = h.fromId ? entityIndex(h.fromId) : null;
      const toIndex = h.toId ? entityIndex(h.toId) : null;
      const to = this.a.set(...gameToGl(h.x, h.y, h.z + CHEST));
      o.group.visible = view.visible(to);

      o.confirm.visible = h.kill;
      if (h.kill) {
        if (!o.confirm.material.map) {
          o.confirm.material.map = this.texKill ??= confirmTexture();
          o.confirm.material.needsUpdate = true;
        }
        o.confirm.position.copy(to);
        o.confirm.scale.setScalar(0.04);
        o.confirm.material.color.set(hitColour(h));
        o.confirm.material.opacity = h.mark;
      }

      o.beam.visible = o.tick.visible = false;
      if (!h.from) continue;
      const ring = toIndex === null ? undefined : view.ring(toIndex);
      if (ring) o.tick.position.copy(ring.position);
      else o.tick.position.set(...gameToGl(h.x, h.y, h.z));
      o.tick.position.y += 0.04;
      const from = this.b.set(...gameToGl(h.from.x, h.from.y, h.from.z + CHEST));
      const dx = from.x - o.tick.position.x, dz = from.z - o.tick.position.z;
      o.tick.visible = h.direction > 0 && Math.hypot(dx, dz) > 1e-6;
      if (o.tick.visible) {
        o.tick.rotation.y = Math.atan2(-dz, dx);
        o.tick.scale.setScalar(ring?.scale ?? scale);
        o.tick.material.color.set(hitColour({ kind: h.kind, kill: false }));
        o.tick.material.opacity = h.direction;
      }

      const focused = (fromIndex !== null && view.focused(fromIndex)) || (toIndex !== null && view.focused(toIndex));
      if (h.vehicle || h.line <= 0 || (!h.kill && !focused)) continue;
      const length = from.distanceTo(to);
      if (length < 1e-6) continue;
      const distance = Math.min(this.camera.position.distanceTo(to), this.camera.position.distanceTo(from));
      const width = Math.max(0.012 * scale, distance * 0.0016) * (h.kill ? 1.6 : 1);
      o.beam.visible = true;
      o.beam.position.copy(from);
      o.beam.quaternion.setFromUnitVectors(AXIS_X, to.sub(from).divideScalar(length));
      o.beam.scale.set(length, width, width);
      const killer = h.kill && fromIndex !== null ? view.colour(fromIndex) : undefined;
      o.beam.material.color.set(killer ?? hitColour(h));
      o.beam.material.opacity = h.line * 0.9;
    }
    for (const [id, o] of this.hits) {
      if (kept.has(id)) continue;
      o.group.visible = false;
      this.hits.delete(id);
      this.spareHits.push(o);
      changed = true;
    }

    const activeProjectiles = new Set<string>();
    for (const p of trails) {
      const end = p.points.at(-1);
      if (!end) continue;
      let o = this.trails.get(p.id);
      if (!o) { o = this.trail(); this.trails.set(p.id, o); }
      activeProjectiles.add(p.id);
      const positions = o.trail.geometry.getAttribute("position") as THREE.BufferAttribute;
      p.points.forEach((point, i) => positions.setXYZ(i, ...gameToGl(...point)));
      o.trail.geometry.setDrawRange(0, p.points.length);
      o.trail.visible = p.points.length > 1;
      positions.needsUpdate = true;
      o.ball.position.set(...gameToGl(...end));
      o.group.visible = view.visible(o.ball.position);
      o.ball.scale.setScalar(Math.max(0.035 * scale, this.camera.position.distanceTo(o.ball.position) * 0.0045));
      const colour = projectileColour(p.kind);
      o.mat.color.set(colour);
      o.trailMat.color.set(colour);
      o.trailMat.opacity = 0.8;
    }
    for (const [id, o] of this.trails) {
      if (activeProjectiles.has(id)) continue;
      o.group.visible = false;
      this.trails.delete(id);
      this.spareTrails.push(o);
      changed = true;
    }
    return changed;
  }

  dispose(): void {
    for (const mark of this.sparks) { mark.removeFromParent(); mark.material.dispose(); }
    this.sparks.length = 0;
    for (const o of [...this.hits.values(), ...this.spareHits]) {
      o.group.removeFromParent();
      o.beam.material.dispose();
      o.tick.material.dispose();
      o.confirm.material.dispose();
    }
    for (const o of [...this.trails.values(), ...this.spareTrails]) { o.trail.geometry.dispose(); o.mat.dispose(); o.trailMat.dispose(); }
    this.geoBeam.dispose(); this.geoBall.dispose(); this.geoTick.dispose(); this.texKill?.dispose();
  }
}
