import * as THREE from "three";
import { SEE_MAX, patchSeeThrough } from "./cutaway";
import { cloneMaterial } from "./players3d";
import { patchReveal, type revealAt } from "./reveal";

function revealUniforms() {
  return {
    uReveal: { value: new THREE.Vector4(0, 0, 0, -1) },
    uRevealBand: { value: 1.5 },
    uRevealCell: { value: 1 },
    uRevealHolo: { value: 0 },
  };
}

/** Shares one camera and set of focus targets across all cutaway materials. */
export class CutawayMaterials {
  readonly uniforms;
  private readonly attached = new WeakSet<THREE.Material>();
  private readonly reveal = { level: revealUniforms(), objects: revealUniforms() };
  private readonly revealSurfaces = new WeakMap<THREE.Material, ReturnType<typeof revealUniforms>>();
  private readonly revealDepth = new WeakSet<THREE.Material>();
  private readonly copies = new Map<THREE.Material, THREE.Material>();

  constructor(private readonly plane: THREE.Plane, eye: THREE.Vector3, private readonly revealEnabled = false) {
    this.uniforms = {
      uSeeEye: { value: eye },
      uSeeTargets: { value: Array.from({ length: SEE_MAX }, () => new THREE.Vector4()) },
      uSeeHalfHeights: { value: new Array<number>(SEE_MAX).fill(0) },
      uSeeCount: { value: 0 },
    };
  }

  /** Apply the height cut, view window and opening sweep to a map material.
   *  Layers and plasma appear as a group and can opt out of the sweep. */
  attach(material: THREE.Material, see = true, group: "level" | "objects" | null = "level") {
    if (this.attached.has(material)) return;
    this.attached.add(material);
    material.clippingPlanes = [this.plane];
    material.clipShadows = true;
    const reveal = this.revealEnabled && group ? this.reveal[group] : null;
    if (reveal) this.revealSurfaces.set(material, reveal);
    if (!see && !reveal) return;
    const before = material.onBeforeCompile, key = material.customProgramCacheKey;
    material.onBeforeCompile = (shader, renderer) => {
      before.call(material, shader, renderer);
      if (see) {
        const cut = patchSeeThrough(shader.vertexShader, shader.fragmentShader);
        if (cut) {
          shader.vertexShader = cut.vertex;
          shader.fragmentShader = cut.fragment;
          Object.assign(shader.uniforms, this.uniforms);
        }
      }
      if (reveal) {
        const sweep = patchReveal(shader.vertexShader, shader.fragmentShader);
        if (sweep) {
          shader.vertexShader = sweep.vertex;
          shader.fragmentShader = sweep.fragment;
          Object.assign(shader.uniforms, reveal);
        }
      }
    };
    material.customProgramCacheKey = () => key.call(material) + (see ? "|see" : "") + (reveal ? "|reveal" : "");
    material.needsUpdate = true;
  }

  /** Three caches a shadow variant per clipped surface. Patch that variant
   *  as it is drawn, so alpha maps and the existing height cut still apply. */
  attachShadow(mesh: THREE.Mesh) {
    if (!this.revealEnabled || !mesh.castShadow) return;
    const previous = mesh.onBeforeShadow;
    mesh.onBeforeShadow = (...args) => {
      previous.apply(mesh, args);
      const [, , , , , depth, group] = args;
      // The renderer passes a geometry group or null here.
      const index = (group as unknown as { materialIndex?: number } | null)?.materialIndex ?? 0;
      const surface = Array.isArray(mesh.material) ? mesh.material[index] : mesh.material;
      const reveal = surface && this.revealSurfaces.get(surface);
      if (!reveal || this.revealDepth.has(depth)) return;
      this.revealDepth.add(depth);
      const before = depth.onBeforeCompile, key = depth.customProgramCacheKey;
      depth.onBeforeCompile = (shader, renderer) => {
        before.call(depth, shader, renderer);
        const sweep = patchReveal(shader.vertexShader, shader.fragmentShader, "shadow");
        if (!sweep) return;
        shader.vertexShader = sweep.vertex;
        shader.fragmentShader = sweep.fragment;
        Object.assign(shader.uniforms, reveal);
      };
      depth.customProgramCacheKey = () => key.call(depth) + "|reveal-shadow";
      depth.needsUpdate = true;
    };
  }

  /** Update both sets of uniforms without changing the compiled programs. */
  setReveal(origin: THREE.Vector3, band: number, cell: number, frame: ReturnType<typeof revealAt>) {
    const { level, objects } = this.reveal;
    level.uReveal.value.set(origin.x, origin.y, origin.z, frame.radius);
    objects.uReveal.value.set(origin.x, origin.y, origin.z, frame.objectsRadius);
    level.uRevealBand.value = objects.uRevealBand.value = band;
    level.uRevealCell.value = objects.uRevealCell.value = cell;
    level.uRevealHolo.value = frame.holo;
    objects.uRevealHolo.value = frame.objectsRadius === 0 ? 0 : frame.holo;
  }

  clearReveal() {
    this.reveal.level.uReveal.value.w = this.reveal.objects.uReveal.value.w = -1;
  }

  /** Use cutaway materials on a runtime placement. Cached model materials
   *  are shared with actors and vehicles, so only the placement gets a copy.
   *  A copy of an exported scenario placement already has the right material. */
  attachModel(root: THREE.Object3D) {
    const materialFor = (source: THREE.Material): THREE.Material => {
      if (this.attached.has(source)) return source;
      let material = this.copies.get(source);
      if (!material) {
        material = cloneMaterial(source);
        this.attach(material, true, "objects");
        this.copies.set(source, material);
      }
      return material;
    };
    root.traverse((object) => {
      const mesh = object as THREE.Mesh;
      if (!mesh.isMesh) return;
      mesh.material = Array.isArray(mesh.material) ? mesh.material.map(materialFor) : materialFor(mesh.material);
      this.attachShadow(mesh);
    });
  }

  dispose() {
    for (const material of this.copies.values()) material.dispose();
    this.copies.clear();
  }
}
