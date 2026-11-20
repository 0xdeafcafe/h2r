import * as THREE from "three";
import { SEE_MAX, patchSeeThrough } from "./cutaway";
import { cloneMaterial } from "./players3d";

/** Shares one camera and set of focus targets across all cutaway materials. */
export class CutawayMaterials {
  readonly uniforms;
  private readonly attached = new WeakMap<THREE.Material, boolean>();
  private readonly copies = new Map<THREE.Material, THREE.Material>();

  constructor(private readonly plane: THREE.Plane, eye: THREE.Vector3) {
    this.uniforms = {
      uSeeEye: { value: eye },
      uSeeTargets: { value: Array.from({ length: SEE_MAX }, () => new THREE.Vector4()) },
      uSeeHalfHeights: { value: new Array<number>(SEE_MAX).fill(0) },
      uSeeCount: { value: 0 },
    };
  }

  /** Apply the height cut and the view window to a material the level owns. */
  attach(material: THREE.Material, see = true) {
    if (this.attached.has(material)) return;
    this.attached.set(material, see);
    material.clippingPlanes = [this.plane];
    material.clipShadows = true;
    if (!see) return;
    const before = material.onBeforeCompile, key = material.customProgramCacheKey;
    material.onBeforeCompile = (shader, renderer) => {
      before.call(material, shader, renderer);
      const out = patchSeeThrough(shader.vertexShader, shader.fragmentShader);
      if (!out) return;
      shader.vertexShader = out.vertex;
      shader.fragmentShader = out.fragment;
      Object.assign(shader.uniforms, this.uniforms);
    };
    material.customProgramCacheKey = () => key.call(material) + "|see";
    material.needsUpdate = true;
  }

  /** Use cutaway materials on a runtime placement. Cached model materials
   *  are shared with actors and vehicles, so only the placement gets a copy.
   *  A copy of an exported scenario placement already has the right material. */
  attachModel(root: THREE.Object3D) {
    const materialFor = (source: THREE.Material): THREE.Material => {
      if (this.attached.get(source)) return source;
      let material = this.copies.get(source);
      if (!material) {
        material = cloneMaterial(source);
        this.attach(material);
        this.copies.set(source, material);
      }
      return material;
    };
    root.traverse((object) => {
      const mesh = object as THREE.Mesh;
      if (!mesh.isMesh) return;
      mesh.material = Array.isArray(mesh.material) ? mesh.material.map(materialFor) : materialFor(mesh.material);
    });
  }

  dispose() {
    for (const material of this.copies.values()) material.dispose();
    this.copies.clear();
  }
}
