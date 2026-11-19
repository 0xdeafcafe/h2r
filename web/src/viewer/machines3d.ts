// Moving a level's machines (lift platforms, doors, big scripted things)
// to where the frames say they are. A frame's machine names the map glb's
// `machines` child it is; that node is then moved, however the glb wrote
// it:
//
// - with a transform of its own (the mesh is in the object's own space,
//   the node holds the placement): the node is simply put where the frame
//   says, keeping its scale;
// - with none (the mesh is already in world space, where the scenario put
//   it): the node is moved by how far the machine is from where it was
//   first seen, which is right as long as that first sight was at rest.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import type { SampledMachinePosition } from "../state/frames";
import { gameToGl } from "../map/views";
import { vehicleBasis } from "./models";

type V3 = readonly [number, number, number];
export interface Placed { x: number; y: number; z: number; forward: V3; up: V3 }

const vx = new THREE.Vector3(), vy = new THREE.Vector3(), vz = new THREE.Vector3();
const pos = new THREE.Vector3(), quat = new THREE.Quaternion(), scale = new THREE.Vector3();
const tmp = new THREE.Matrix4();

/** A machine's place and facing as a matrix, in the scene's axes. */
export function machineMatrix(m: Placed, out = new THREE.Matrix4()): THREE.Matrix4 {
  const b = vehicleBasis(m.forward as [number, number, number], m.up as [number, number, number]);
  out.makeBasis(vx.set(...b.x), vy.set(...b.y), vz.set(...b.z));
  return out.setPosition(...gameToGl(m.x, m.y, m.z));
}

export function isIdentity(m: THREE.Matrix4, eps = 1e-6): boolean {
  const e = m.elements;
  for (let i = 0; i < 16; i++) if (Math.abs(e[i]! - (i % 5 === 0 ? 1 : 0)) > eps) return false;
  return true;
}

export interface MachineNode {
  obj: THREE.Object3D;
  /** The node's transform in the glb. */
  rest: THREE.Matrix4;
  /** Whether the glb left the node untransformed (its mesh is in world space). */
  baked: boolean;
  /** Where the machine was when first seen: what `rest` is taken to show. */
  base: THREE.Matrix4 | null;
  device: DeviceAnimation | null;
}

/** Takes charge of a `machines` child: from now on its matrix is set by hand. */
export function machineNode(obj: THREE.Object3D, clips?: ReadonlyMap<string, THREE.AnimationClip>): MachineNode {
  obj.updateMatrix();
  obj.matrixAutoUpdate = false;
  const rest = obj.matrix.clone();
  const device = clips ? deviceAnimation(obj, clips) : null;
  return { obj, rest, baked: !device && isIdentity(rest), base: null, device };
}

/** Puts a node where its machine is now. Returns whether it moved. */
export function placeMachine(n: MachineNode, now: THREE.Matrix4): boolean {
  const m = n.obj.matrix;
  tmp.copy(m);
  if (n.baked) {
    n.base ??= now.clone();
    // now × base⁻¹ carries the first-seen place to the present one; the mesh rides along.
    m.copy(n.base).invert().premultiply(now).multiply(n.rest);
  } else {
    n.rest.decompose(pos, quat, scale);
    now.decompose(pos, quat, vx);
    m.compose(pos, quat, scale);
  }
  if (m.equals(tmp)) return false;
  n.obj.matrixWorldNeedsUpdate = true;
  return true;
}


interface DeviceAnimation {
  index: number;
  loop: boolean;
  mixer: THREE.AnimationMixer;
  action: THREE.AnimationAction;
  position: number | undefined;
}

function deviceAnimation(obj: THREE.Object3D, clips: ReadonlyMap<string, THREE.AnimationClip>): DeviceAnimation | null {
  const info = obj.userData.device;
  if (!info || !Number.isInteger(info.index) || info.index < 0 || info.index > 0x7fff) return null;
  const clip = clips.get(`device_position_${info.index}`);
  if (!clip || !(clip.duration > 0)) return null;
  // A device can move outside its rest-pose bounds.
  obj.traverse((o) => {
    if ((o as THREE.SkinnedMesh).isSkinnedMesh) o.frustumCulled = false;
  });
  const mixer = new THREE.AnimationMixer(obj);
  const action = mixer.clipAction(clip);
  action.setLoop(THREE.LoopOnce, 1);
  action.clampWhenFinished = true;
  return { index: info.index, loop: info.loop === true, mixer, action, position: undefined };
}

/** Scrub the device's own clip to the sampled travel. An absent sample
 *  restores the bind pose, including when seeking into an older recording. */
export function placeDevice(n: MachineNode, samples?: ReadonlyMap<number, SampledMachinePosition>): boolean {
  const d = n.device;
  if (!d) return false;
  const sample = samples?.get(d.index);
  let position: number | undefined;
  if (sample) {
    let delta = sample.to - sample.from;
    if (d.loop && Math.abs(delta) > 0.5) delta -= Math.sign(delta);
    position = sample.from + delta * sample.mix;
    if (position < 0) position += 1;
    else if (position > 1) position -= 1;
  }
  if (position === d.position) return false;
  d.position = position;
  if (position === undefined) {
    d.action.stop();
  } else {
    d.action.play();
    d.action.paused = true;
    d.action.time = position * d.action.getClip().duration;
    d.mixer.update(0);
  }
  return true;
}
