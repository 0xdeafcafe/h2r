import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { machineMatrix, machineNode, placeDevice, placeMachine } from "../src/viewer/machines3d";

function rig(index: number, loop = false) {
  const obj = new THREE.Group();
  obj.userData.device = { index, loop };
  const bone = new THREE.Bone();
  bone.name = "platform";
  obj.add(bone);
  const geometry = new THREE.BufferGeometry()
    .setAttribute("position", new THREE.Float32BufferAttribute([0, 0, 0], 3))
    .setAttribute("skinIndex", new THREE.Uint16BufferAttribute([0, 0, 0, 0], 4))
    .setAttribute("skinWeight", new THREE.Float32BufferAttribute([1, 0, 0, 0], 4));
  const mesh = new THREE.SkinnedMesh(geometry);
  obj.add(mesh);
  mesh.bind(new THREE.Skeleton([bone]), new THREE.Matrix4());
  const clip = new THREE.AnimationClip(`device_position_${index}`, 1, [
    new THREE.VectorKeyframeTrack("platform.position", [0, 1], [0, 0, 0, 0, 4, 0]),
  ]);
  return { node: machineNode(obj, new Map([[clip.name, clip]])), mesh };
}

function sample(from: number, to: number, mix: number) {
  return new Map([[7, { from, to, mix }]]);
}

function vertex(mesh: THREE.SkinnedMesh) {
  mesh.parent!.updateMatrixWorld(true);
  return mesh.getVertexPosition(0, new THREE.Vector3()).applyMatrix4(mesh.matrixWorld).toArray();
}

describe("applying machine telemetry to a device rig", () => {
  test("the device follows samples with a fixed root and seeks both ways", () => {
    const { node, mesh } = rig(7);
    const other = rig(8);
    expect(placeDevice(node, sample(0, 1, 0.25))).toBe(true);
    expect(vertex(mesh)).toEqual([0, 1, 0]);
    expect(placeDevice(other.node, sample(0, 1, 0.25))).toBe(false);
    expect(vertex(other.mesh)).toEqual([0, 0, 0]);
    expect(placeDevice(node, sample(0, 1, 0.25))).toBe(false);
    expect(placeDevice(node, sample(1, 1, 0))).toBe(true);
    expect(vertex(mesh)).toEqual([0, 4, 0]);
    expect(placeDevice(node, sample(0, 1, 0.5))).toBe(true);
    expect(vertex(mesh)).toEqual([0, 2, 0]);

    // Root pose changes and animation travel compose independently.
    expect(placeMachine(node, machineMatrix({ x: 5, y: 3, z: 2, forward: [1, 0, 0], up: [0, 0, 1] }))).toBe(true);
    expect(vertex(mesh)).toEqual([5, 4, -3]);
    expect(placeDevice(node)).toBe(true);
    expect(vertex(mesh)).toEqual([5, 2, -3]);
    expect(placeDevice(node)).toBe(false);
  });

  test("a looping rig takes the short way across the seam", () => {
    const { node, mesh } = rig(7, true);
    expect(placeDevice(node, sample(0.9, 0.1, 0.75))).toBe(true);
    expect(vertex(mesh)[1]).toBeCloseTo(0.2);
    expect(placeDevice(node, sample(0.1, 0.9, 0.75))).toBe(true);
    expect(vertex(mesh)[1]).toBeCloseTo(3.8);
  });
});
