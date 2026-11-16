// A map's teleporters, their links and its gravity lifts in the 3D view:
// teleporters as upright rings in their role's colour, links as arcs
// through the air with an arrowhead where they arrive, lifts as
// translucent columns along their push with an arrow at the far end. Built
// once per map from map/features.ts.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { gameToGl } from "../map/views";
import { LIFT_COLOUR, TELEPORTER_COLOUR, liftEnd, linkEnds, type MapFeatures } from "../map/features";

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

  for (const l of f.lifts) {
    const from = new THREE.Vector3(...gameToGl(l.x, l.y, l.z)), to = new THREE.Vector3(...gameToGl(...liftEnd(l)));
    const dir = to.clone().sub(from).normalize();
    const g = new THREE.Group();
    g.name = "lift:" + l.id;
    g.position.copy(from);
    g.quaternion.setFromUnitVectors(UP, dir);
    const column = new THREE.Mesh(
      new THREE.CylinderGeometry(l.radius, l.radius, l.length, 28, 1, true).translate(0, l.length / 2, 0),
      flat(LIFT_COLOUR, 0.2, { side: THREE.DoubleSide }),
    );
    const rim = new THREE.Mesh(new THREE.TorusGeometry(l.radius, Math.max(0.03, l.radius * 0.05), 6, 36).rotateX(Math.PI / 2), flat(LIFT_COLOUR, 0.9));
    const top = rim.clone();
    top.position.y = l.length;
    // The arrow: a shaft up the middle and a head at the far end.
    const shaft = new THREE.Mesh(new THREE.CylinderGeometry(l.radius * 0.07, l.radius * 0.07, l.length * 0.8, 6).translate(0, l.length * 0.45, 0), flat(LIFT_COLOUR, 0.75));
    const head = new THREE.Mesh(new THREE.ConeGeometry(l.radius * 0.32, l.radius * 0.8, 12).translate(0, l.length * 0.85 + l.radius * 0.4, 0), flat(LIFT_COLOUR, 0.9));
    g.add(column, rim, top, shaft, head);
    root.add(g);
  }
  root.traverse((o) => { o.renderOrder = 3; });
  return root;
}
