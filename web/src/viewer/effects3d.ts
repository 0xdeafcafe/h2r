// Momentary things in the 3D view: damage (a beam from attacker to victim,
// a hit marker that keeps its size on screen, and the amount floating up)
// and recorded projectiles in flight. The clock supplies each projectile
// path, and pools keep geometry and materials reusable.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { CSS2DObject } from "three/examples/jsm/renderers/CSS2DRenderer.js";
import type { VisibleFilmEffect } from "./film-world";
import { gameToGl } from "../map/views";
import { hitColour, type LiveHit } from "../map/events";
import { MAX_TRAIL_POINTS, projectileColour, type ProjectileTrail } from "../state/projectile-trails";

interface HitObj { group: THREE.Group; beam: THREE.Mesh; beamMat: THREE.MeshBasicMaterial; mark: THREE.Sprite; markMat: THREE.SpriteMaterial; label: CSS2DObject; el: HTMLDivElement }
interface TrailObj { group: THREE.Group; ball: THREE.Mesh; mat: THREE.MeshBasicMaterial; trail: THREE.Line; trailMat: THREE.LineBasicMaterial }

const AXIS_X = new THREE.Vector3(1, 0, 0);
/** A hit lands about chest height on whoever it names (game units above their feet). */
export const CHEST = 0.45;

/** The hit marker: four ticks around a gap; a kill adds a ring. White, tinted by its material. */
function markTexture(kill: boolean): THREE.Texture {
  const c = document.createElement("canvas");
  c.width = c.height = 64;
  const ctx = c.getContext("2d")!;
  ctx.lineCap = "round";
  for (const [style, w] of [["rgba(0,0,0,.85)", 9], ["#fff", 5]] as const) {
    ctx.strokeStyle = style; ctx.lineWidth = w;
    ctx.beginPath();
    for (const [dx, dy] of [[1, 1], [1, -1], [-1, 1], [-1, -1]] as const) { ctx.moveTo(32 + dx * 9, 32 + dy * 9); ctx.lineTo(32 + dx * 19, 32 + dy * 19); }
    if (kill) { ctx.moveTo(32 + 26, 32); ctx.arc(32, 32, 26, 0, Math.PI * 2); }
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
  private texHit: THREE.Texture | null = null;
  private texKill: THREE.Texture | null = null;
  private a = new THREE.Vector3();
  private b = new THREE.Vector3();

  constructor(private scene: THREE.Scene, private camera: THREE.Camera) {}

  /** Damage-report origins and the final observed segment of a projectile.
   * Deletion alone draws no explosion. Age is measured on the film clock,
   * so these effects freeze, rewind and resume with the simulation. */
  film(events: readonly VisibleFilmEffect[], still: boolean) {
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
      mark.visible = true;
      mark.material.color.setHex(blast ? 0xff932f : terminal ? 0xffe0aa : 0xffc76b);
      mark.material.opacity = (1 - life) * (still ? .45 : .82);
      changed = true;
    }
    for (let i = used; i < this.sparks.length && this.sparks[i]!.visible; i++) {
      this.sparks[i]!.visible = false;
      changed = true;
    }
    return changed;
  }

  private hit(): HitObj {
    const old = this.spareHits.pop();
    if (old) { old.group.visible = true; old.el.style.display = ""; return old; }
    const beamMat = new THREE.MeshBasicMaterial({ transparent: true, depthWrite: false, fog: false, blending: THREE.AdditiveBlending, side: THREE.DoubleSide });
    const beam = new THREE.Mesh(this.geoBeam, beamMat);
    beam.renderOrder = 4;
    // Seen through walls: who was hit matters more than what is in the way.
    const markMat = new THREE.SpriteMaterial({ transparent: true, depthTest: false, depthWrite: false, fog: false, sizeAttenuation: false, toneMapped: false });
    const mark = new THREE.Sprite(markMat);
    mark.renderOrder = 6;
    const el = document.createElement("div");
    el.className = "v3-hit";
    const label = new CSS2DObject(el);
    label.center.set(0.5, 1);
    const group = new THREE.Group();
    group.add(beam, mark, label);
    this.scene.add(group);
    return { group, beam, beamMat, mark, markMat, label, el };
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

  /** Draws this moment's damage and recorded projectiles. `S` is the scene's marker
   *  scale; `still` (reduced motion) keeps numbers in place. Returns
   *  whether anything is (or just stopped being) on screen. */
  update(hits: readonly LiveHit[], trails: readonly ProjectileTrail[], S: number, still: boolean): boolean {
    let changed = hits.length > 0 || trails.length > 0;
    const kept = new Set<number>();
    for (const h of hits) {
      let o = this.hits.get(h.id);
      if (!o) { o = this.hit(); this.hits.set(h.id, o); }
      kept.add(h.id);
      const colour = hitColour(h);
      const to = this.a.set(...gameToGl(h.x, h.y, h.z + CHEST));
      const far = this.camera.position.distanceTo(to);
      o.mark.position.copy(to);
      o.mark.scale.setScalar((h.kill ? 0.04 : 0.027) * (h.age < 90 ? 1.5 - h.age / 180 : 1));
      if (!this.texHit) { this.texHit = markTexture(false); this.texKill = markTexture(true); }
      o.markMat.map = h.kill ? this.texKill : this.texHit;
      o.markMat.color.set(colour);
      o.markMat.opacity = Math.max(0, Math.min(1, h.mark));
      const line = still ? 0.5 * h.mark : h.line;
      o.beam.visible = !!h.from && line > 0;
      if (h.from && line > 0) {
        const from = this.b.set(...gameToGl(h.from.x, h.from.y, h.from.z + CHEST));
        const len = from.distanceTo(to);
        o.beam.position.copy(from);
        if (len > 1e-6) o.beam.quaternion.setFromUnitVectors(AXIS_X, to.clone().sub(from).divideScalar(len));
        const w = Math.max(0.012 * S, Math.min(far, this.camera.position.distanceTo(from)) * 0.0016) * (h.kill ? 1.6 : 1);
        o.beam.scale.set(len, w, w);
        o.beamMat.color.set(colour);
        o.beamMat.opacity = line * 0.9;
      }
      o.label.position.copy(to);
      o.label.position.y += far * 0.03 * (1 + (still ? 0 : h.rise * 1.6));
      if (o.el.textContent !== h.text) o.el.textContent = h.text;
      o.el.style.color = colour;
      o.el.style.opacity = String(Math.max(0, Math.min(1, h.mark)));
      o.el.classList.toggle("v3-hit--kill", h.kill);
      o.label.visible = h.text !== "";
    }
    for (const [id, o] of this.hits) {
      if (kept.has(id)) continue;
      o.group.visible = false;
      o.el.style.display = "none";
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
      o.ball.scale.setScalar(Math.max(0.035 * S, this.camera.position.distanceTo(o.ball.position) * 0.0045));
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
    for (const o of [...this.hits.values(), ...this.spareHits]) { o.label.removeFromParent(); o.el.remove(); o.beamMat.dispose(); o.markMat.dispose(); }
    for (const o of [...this.trails.values(), ...this.spareTrails]) { o.trail.geometry.dispose(); o.mat.dispose(); o.trailMat.dispose(); }
    this.geoBeam.dispose(); this.geoBall.dispose();
    this.texHit?.dispose(); this.texKill?.dispose();
  }
}
