// A map's teleporters, their links and its gravity lifts in the 3D view:
// teleporters as upright rings in their role's colour, links as arcs
// through the air with an arrowhead where they arrive, lifts as small
// guides aligned to the emitter. Built once per map from map/features.ts.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { gameToGl, type Vec3 } from "../map/views";
import { LIFT_COLOUR, TELEPORTER_COLOUR, linkEnds, type Lift, type MapFeatures } from "../map/features";
import type { SampledMachine } from "../state/frames";
import type { ObjectEntry } from "./objects3d";

const UP = new THREE.Vector3(0, 1, 0);
/** A teleporter ring's radius, game units: about a doorway. */
export const RING = 0.5;

const flat = (colour: string, opacity: number, more: THREE.MeshBasicMaterialParameters = {}) =>
  new THREE.MeshBasicMaterial({ color: colour, transparent: true, opacity, depthWrite: false, fog: false, ...more });

/** Everything in one group; `k` scales the marks up on a big level so they can be found. */
export function buildFeatures(f: MapFeatures, k = 1): THREE.Group {
  const root = new THREE.Group();
  root.name = "features";
  const R = RING * k;
  const ringGeo = new THREE.TorusGeometry(R, R * 0.09, 8, 40);
  const discGeo = new THREE.CircleGeometry(R * 0.92, 32);
  const padGeo = new THREE.RingGeometry(R * 0.75, R * 1.05, 32).rotateX(-Math.PI / 2);
  const centre = (t: { x: number; y: number; z: number }) => new THREE.Vector3(...gameToGl(t.x, t.y, t.z)).add(new THREE.Vector3(0, R * 1.12, 0));

  for (const t of f.teleporters) {
    const colour = TELEPORTER_COLOUR[t.role];
    const g = new THREE.Group();
    g.name = "teleporter:" + t.id;
    const ring = new THREE.Mesh(ringGeo, flat(colour, 0.95));
    const disc = new THREE.Mesh(discGeo, flat(colour, t.role === "receiver" ? 0.1 : 0.24, { side: THREE.DoubleSide }));
    ring.position.y = disc.position.y = R * 1.12;
    const pad = new THREE.Mesh(padGeo, flat(colour, 0.6, { side: THREE.DoubleSide }));
    pad.position.y = 0.03 * k;
    g.add(ring, disc, pad);
    g.position.set(...gameToGl(t.x, t.y, t.z));
    // The ring's face (+z as made) looks along the teleporter's facing: game (cos, sin, 0) is scene (cos, 0, -sin).
    g.rotation.y = t.facing + Math.PI / 2;
    root.add(g);
  }

  const linkMat = flat("#d6eeff", 0.6);
  const headGeo = new THREE.ConeGeometry(R * 0.2, R * 0.5, 10);
  for (const l of linkEnds(f)) {
    const a = centre(l.from), b = centre(l.to);
    // The same arc the 2D maps' data describes (map/features.ts linkArc), with the scene's up.
    const rise = Math.max(1, a.distanceTo(b) * 0.2);
    const pts = Array.from({ length: 24 }, (_, i) => {
      const u = i / 23, p = new THREE.Vector3().lerpVectors(a, b, u);
      p.y += rise * 4 * u * (1 - u);
      return p;
    });
    const curve = new THREE.CatmullRomCurve3(pts);
    const tube = new THREE.Mesh(new THREE.TubeGeometry(curve, 48, R * 0.045, 6), linkMat);
    tube.name = `link:${l.from.id}:${l.to.id}`;
    root.add(tube);
    for (const end of l.both ? [1, 0] : [1]) {
      const head = new THREE.Mesh(headGeo, linkMat);
      const u = end ? 0.94 : 0.06;
      head.position.copy(curve.getPoint(u));
      head.quaternion.setFromUnitVectors(UP, curve.getTangent(u).multiplyScalar(end ? 1 : -1).normalize());
      root.add(head);
    }
  }

  for (const l of f.lifts) root.add(new LiftGuide(l));
  root.traverse((o) => { o.renderOrder = 3; });
  return root;
}

const guidePosition = new THREE.Vector3();
const guideAxis = new THREE.Vector3();
const guideRotation = new THREE.Quaternion();

class LiftGuide extends THREE.Group {
  private readonly anchor: Vec3;
  private readonly machineNode: string | undefined;

  constructor(private readonly lift: Lift) {
    super();
    this.name = "lift:" + lift.id;
    this.anchor = [lift.x, lift.y, lift.z];
    const at = lift.placement;
    this.machineNode = at?.kind === "mach" ? `${at.stem}_${at.index}` : undefined;
    // Bounds estimate reach and width, but the guide only marks the emitter.
    const radius = Math.min(lift.radius * 0.65, 0.75);
    const length = Math.min(lift.length * 0.65, 1.5);
    const rim = new THREE.Mesh(
      new THREE.RingGeometry(radius * 0.86, radius, 28).rotateX(-Math.PI / 2),
      flat(LIFT_COLOUR, 0.42, { side: THREE.DoubleSide }),
    );
    rim.position.y = 0.025;
    const direction = flat(LIFT_COLOUR, 0.5);
    const stem = new THREE.Mesh(new THREE.CylinderGeometry(0.022, 0.022, length * 0.65, 6), direction);
    stem.position.y = length * 0.325;
    const head = new THREE.Mesh(new THREE.ConeGeometry(Math.min(radius * 0.28, 0.13), length * 0.35, 8), direction);
    head.position.y = length * 0.825;
    this.add(rim, stem, head);
    this.place();
  }

  place(machines?: readonly SampledMachine[], objects?: readonly ObjectEntry[]): boolean {
    const at = this.lift.placement;
    const machine = this.machineNode ? machines?.find(m => m.node === this.machineNode) : undefined;
    const object = at && !machine ? objects?.find(o => o.k === at.kind && o.i === at.index) : undefined;
    const pos = machine ? [machine.x, machine.y, machine.z] as Vec3 : object?.p ?? this.anchor;
    // The ring is rotationally symmetric. Its normal and the arrow both
    // follow the emitter's local up, including pitch and roll.
    const up = machine?.up ?? object?.u ?? this.lift.dir;
    guidePosition.set(...gameToGl(...pos));
    guideAxis.set(...gameToGl(...up)).normalize();
    guideRotation.setFromUnitVectors(UP, guideAxis);
    const visible = !object?.gone;
    if (this.visible === visible && this.position.equals(guidePosition) && this.quaternion.equals(guideRotation)) return false;
    this.visible = visible;
    this.position.copy(guidePosition);
    this.quaternion.copy(guideRotation);
    return true;
  }
}

export function placeLiftGuides(group: THREE.Group, machines?: readonly SampledMachine[], objects?: readonly ObjectEntry[] | null): boolean {
  let moved = false;
  for (const child of group.children) if (child instanceof LiftGuide && child.place(machines, objects ?? undefined)) moved = true;
  return moved;
}
