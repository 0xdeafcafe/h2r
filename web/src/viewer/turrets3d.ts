// Vehicle turrets on a three.js model (see turrets.ts for the parts that
// need no three.js): finding a vehicle glb's turret nodes, the angles that
// point one along an aim, and posing it.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { gameToGl, glToGame } from "../map/views";
import { turretAngles } from "./turrets";

/** A turret on a vehicle model: its yaw node and gun (pitch) nodes, each
 *  with its rest rotation, and the angles drawn now (eased). */
export interface TurretObj {
  kind: string;
  yaw: THREE.Object3D;
  rest: THREE.Quaternion;
  guns: Array<{ node: THREE.Object3D; rest: THREE.Quaternion }>;
  now: { yaw: number; pitch: number };
}

/** A vehicle model's turrets: its `turret_<n>` nodes (their extras name
 *  the turret tag) and their `turret_<n>_pitch_<k>` children, in order. */
export function findTurrets(model: THREE.Object3D): TurretObj[] {
  const out: TurretObj[] = [];
  model.traverse((o) => {
    if (!/^turret_\d+$/.test(o.name)) return;
    const guns: TurretObj["guns"] = [];
    o.traverse((c) => { if (c !== o && /^turret_\d+_pitch_\d+$/.test(c.name)) guns.push({ node: c, rest: c.quaternion.clone() }); });
    const kind = typeof o.userData.turret === "string" ? o.userData.turret : "";
    out.push({ kind, yaw: o, rest: o.quaternion.clone(), guns, now: { yaw: 0, pitch: 0 } });
  });
  return out;
}

const UP = new THREE.Vector3(0, 1, 0);
/** A gun's side axis in glTF: the game's +y (left) is glTF -z, and pitching
 *  up is a turn about the game's -y, so about glTF +z. */
const SIDE = new THREE.Vector3(0, 0, 1);
const q = new THREE.Quaternion();
const d = new THREE.Vector3();

/** The yaw and pitch that point turret `t` along `aim` (game coordinates),
 *  measured in its rest frame where the model now stands in the world (its
 *  parents' world matrices must be current). */
export function turretWant(t: TurretObj, aim: readonly [number, number, number]): { yaw: number; pitch: number } {
  t.yaw.quaternion.copy(t.rest);
  t.yaw.updateWorldMatrix(true, false);
  t.yaw.getWorldQuaternion(q).invert();
  const [x, y, z] = gameToGl(aim[0], aim[1], aim[2]);
  d.set(x, y, z).applyQuaternion(q);
  return turretAngles(glToGame(d.x, d.y, d.z));
}

/** Poses turret `t` at its `now` angles: the yaw node turned about its up,
 *  each gun about its side. */
export function poseTurret(t: TurretObj): void {
  t.yaw.quaternion.copy(t.rest).multiply(q.setFromAxisAngle(UP, t.now.yaw));
  q.setFromAxisAngle(SIDE, t.now.pitch);
  for (const g of t.guns) g.node.quaternion.copy(g.rest).multiply(q);
}
