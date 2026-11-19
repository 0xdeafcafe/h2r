// The ambient occlusion pass for the 3D view: three's GTAO, reading the
// depth the scene was just drawn with (the composer's render target has a
// depth texture) and working its normals out from that. Its own depth and
// normal pass drew the whole scene a second time with one material: as many
// draw calls again, and with skinned, instanced and plain meshes sharing
// that material, three looked their program up again at nearly every one
// (half the frame's CPU time on a campaign level with its AI).
//
// Without a depth texture it falls back to that pass, leaving out what is
// not a surface: anything see-through that writes no depth (glows, light
// volumes, decals, the sky drawn around the camera, tracers and flashes)
// would otherwise be a solid shape there and darken what is around it. The
// scene's depth already leaves those out.
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
  /** `depth`: the scene's depth, as the composer's render target holds it
   *  (each of its two targets has its own; the one read is used). */
  constructor(scene: THREE.Scene, camera: THREE.Camera, width: number, height: number, depth?: THREE.DepthTexture) {
    super(scene, camera, width, height);
    if (depth) {
      this.setGBuffer(depth);
      // Its own depth and normal target is not drawn into now.
      (this as unknown as { normalRenderTarget?: THREE.WebGLRenderTarget }).normalRenderTarget?.dispose();
    }
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

  override render(renderer: THREE.WebGLRenderer, writeBuffer: THREE.WebGLRenderTarget, readBuffer: THREE.WebGLRenderTarget, deltaTime: number, maskActive: boolean) {
    const depth = readBuffer.depthTexture;
    const self = this as unknown as { _renderGBuffer: boolean };
    if (!self._renderGBuffer && depth && depth !== this.depthTexture) {
      this.depthTexture = depth;
      this.gtaoMaterial.uniforms.tDepth!.value = depth;
      this.pdMaterial.uniforms.tDepth!.value = depth;
      this.depthRenderMaterial.uniforms.tDepth!.value = depth;
    }
    super.render(renderer, writeBuffer, readBuffer, deltaTime, maskActive);
  }
}
