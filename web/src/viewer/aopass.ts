// The ambient occlusion pass for the 3D view: three's GTAO, except that
// its depth and normal pass leaves out what is not a surface. It draws the
// whole scene with one opaque material of its own, so anything see-through
// that writes no depth (glows, light volumes, decals, the sky drawn around
// the camera, tracers and flashes) would otherwise be a solid shape there
// and darken what is around it.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { GTAOPass } from "three/examples/jsm/postprocessing/GTAOPass.js";

/** Whether the occlusion pass leaves an object out: marked `noAo`, or a
 *  mesh whose materials are all blended and write no depth. */
export function skipsOcclusion(o: THREE.Object3D): boolean {
  if (o.userData?.noAo) return true;
  const mesh = o as THREE.Mesh;
  if (!mesh.isMesh) return false;
  const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
  return mats.length > 0 && mats.every((m) => m.transparent && !m.depthWrite);
}

export class LevelAoPass extends GTAOPass {
  constructor(scene: THREE.Scene, camera: THREE.Camera, width: number, height: number) {
    super(scene, camera, width, height);
    const self = this as unknown as { _overrideVisibility: () => void; _visibilityCache: THREE.Object3D[] };
    const base = self._overrideVisibility.bind(this);
    self._overrideVisibility = () => {
      base();
      const cache = self._visibilityCache;
      scene.traverse((o) => {
        if (o.visible && skipsOcclusion(o)) {
          o.visible = false;
          cache.push(o);
        }
      });
    };
  }
}
