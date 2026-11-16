// Momentary things in the 3D view: damage (a beam from attacker to victim,
// a hit marker that keeps its size on screen, and the amount floating up)
// and grenades in flight (the arc of the throw). What to draw and for how
// long comes from map/events.ts; this only puts it in the scene, from
// pools, so a firefight makes no garbage.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { CSS2DObject } from "three/examples/jsm/renderers/CSS2DRenderer.js";
import { gameToGl } from "../map/views";
import { grenadeColour, grenadeTrail, hitColour, type Grenade, type LiveHit } from "../map/events";

interface HitObj { group: THREE.Group; beam: THREE.Mesh; beamMat: THREE.MeshBasicMaterial; mark: THREE.Sprite; markMat: THREE.SpriteMaterial; label: CSS2DObject; el: HTMLDivElement }
interface NadeObj { group: THREE.Group; ball: THREE.Mesh; mat: THREE.MeshBasicMaterial; trail: THREE.Line; trailMat: THREE.LineBasicMaterial }

const AXIS_X = new THREE.Vector3(1, 0, 0);
const TRAIL_POINTS = 14;
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
  private hits = new Map<number, HitObj>();
  private spareHits: HitObj[] = [];
  private nades = new Map<number, NadeObj>();
  private spareNades: NadeObj[] = [];
  private geoBeam = new THREE.CylinderGeometry(1, 1, 1, 5, 1, true).rotateZ(-Math.PI / 2).translate(0.5, 0, 0);
  private geoBall = new THREE.SphereGeometry(1, 10, 8);
  private texHit: THREE.Texture | null = null;
  private texKill: THREE.Texture | null = null;
  private a = new THREE.Vector3();
  private b = new THREE.Vector3();

  constructor(private scene: THREE.Scene, private camera: THREE.Camera) {}

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

  private nade(): NadeObj {
    const old = this.spareNades.pop();
    if (old) { old.group.visible = true; return old; }
    const mat = new THREE.MeshBasicMaterial({ transparent: true, fog: false });
    const ball = new THREE.Mesh(this.geoBall, mat);
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(TRAIL_POINTS * 3), 3));
    const trailMat = new THREE.LineBasicMaterial({ transparent: true, depthWrite: false, fog: false });
    const trail = new THREE.Line(geo, trailMat);
    trail.frustumCulled = false;
    ball.renderOrder = trail.renderOrder = 4;
    const group = new THREE.Group();
    group.add(ball, trail);
    this.scene.add(group);
    return { group, ball, mat, trail, trailMat };
  }

  /** Draws this moment's damage and grenades. `S` is the scene's marker
   *  scale; `still` (reduced motion) keeps numbers in place. Returns
   *  whether anything is (or just stopped being) on screen. */
  update(hits: readonly LiveHit[], grenades: ReadonlyArray<Grenade & { age: number; alpha: number }>, S: number, still: boolean): boolean {
    let changed = hits.length > 0 || grenades.length > 0;
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

    kept.clear();
    for (const g of grenades) {
      let o = this.nades.get(g.id);
      if (!o) { o = this.nade(); this.nades.set(g.id, o); }
      kept.add(g.id);
      const pts = grenadeTrail(g, g.age, TRAIL_POINTS);
      const pos = o.trail.geometry.getAttribute("position") as THREE.BufferAttribute;
      pts.forEach((p, i) => { const [x, y, z] = gameToGl(...p); pos.setXYZ(i, x, y, z); });
      pos.needsUpdate = true;
      const end = pts[pts.length - 1]!;
      o.ball.position.set(...gameToGl(...end));
      o.ball.scale.setScalar(Math.max(0.035 * S, this.camera.position.distanceTo(o.ball.position) * 0.0045));
      const colour = grenadeColour(g.type);
      o.mat.color.set(colour); o.trailMat.color.set(colour);
      o.mat.opacity = g.alpha; o.trailMat.opacity = g.alpha * 0.8;
    }
    for (const [id, o] of this.nades) {
      if (kept.has(id)) continue;
      o.group.visible = false;
      this.nades.delete(id);
      this.spareNades.push(o);
      changed = true;
    }
    return changed;
  }

  dispose(): void {
    for (const o of [...this.hits.values(), ...this.spareHits]) { o.label.removeFromParent(); o.el.remove(); o.beamMat.dispose(); o.markMat.dispose(); }
    for (const o of [...this.nades.values(), ...this.spareNades]) { o.trail.geometry.dispose(); o.mat.dispose(); o.trailMat.dispose(); }
    this.geoBeam.dispose(); this.geoBall.dispose();
    this.texHit?.dispose(); this.texKill?.dispose();
  }
}
