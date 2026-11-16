// A level's layers in the 3D view: decals and overlays on its surfaces, and
// effects in the air (glows, light shafts and volumes, waterfalls). The map
// glb keeps them in `decals_<n>` and `effects_<n>` nodes under each zone;
// each material's extras say how it blends (docs/live/live-art.md, "Layers").
// `layerOf` and `layerLook` are pure; `dressLayer` swaps a loaded mesh's
// material for one that blends that way.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";

export type LayerGroup = "decal" | "effect";
export type LayerBlend = "alpha" | "multiply" | "double" | "test" | "additive";
export interface LayerSpec { group: LayerGroup; blend: LayerBlend }

/** A material's layer from its glTF extras (three's `userData`), or null
 *  for ordinary level materials. */
export function layerOf(extras: unknown): LayerSpec | null {
  const e = (extras ?? {}) as Record<string, unknown>;
  const group = e.layer, blend = e.blend;
  if (group !== "decal" && group !== "effect") return null;
  const b: LayerBlend = blend === "multiply" || blend === "double" || blend === "test" || blend === "additive" ? blend : "alpha";
  return { group, blend: b };
}

/** How a layer is drawn. Decals sit just in front of the surface they are
 *  on (a polygon offset), cast no shadow and write no depth unless cut out;
 *  layers that multiply, double or add are unlit and leave fog to what is
 *  behind them (it is already fogged; a glow is light, not a surface);
 *  effects are two-sided. Drawn after the level: decals first, then
 *  effects. */
export interface LayerLook {
  unlit: boolean;
  blending: "normal" | "multiply" | "double" | "add";
  transparent: boolean;
  depthWrite: boolean;
  alphaTest: number;
  polygonOffset: boolean;
  doubleSided: boolean;
  fog: boolean;
  renderOrder: number;
}

export function layerLook(l: LayerSpec): LayerLook {
  const decal = l.group === "decal";
  return {
    unlit: l.blend === "multiply" || l.blend === "double" || l.blend === "additive",
    blending: l.blend === "additive" ? "add" : l.blend === "multiply" || l.blend === "double" ? l.blend : "normal",
    transparent: l.blend !== "test",
    depthWrite: l.blend === "test",
    alphaTest: l.blend === "test" ? 0.5 : 0,
    polygonOffset: decal,
    doubleSided: !decal,
    fog: l.blend === "alpha" || l.blend === "test",
    renderOrder: decal ? 1 : 2,
  };
}

/** How much of an additive layer's picture is added. */
export const ADD_STRENGTH = 0.3;

/** Gives a layer mesh its look: a new material (the glb's is disposed)
 *  keeping its picture and base colour. Returns the layer, or null when the
 *  mesh is not one. */
export function dressLayer(mesh: THREE.Mesh): LayerSpec | null {
  const src = mesh.material as THREE.Material & { map?: THREE.Texture | null; color?: THREE.Color };
  if (Array.isArray(mesh.material)) return null;
  const spec = layerOf(src.userData);
  if (!spec) return null;
  const look = layerLook(spec);
  const params = {
    name: src.name,
    map: src.map ?? null,
    color: src.color?.clone() ?? new THREE.Color(1, 1, 1),
    transparent: look.transparent,
    depthWrite: look.depthWrite,
    alphaTest: look.alphaTest,
    polygonOffset: look.polygonOffset,
    // Pulled towards the camera a little, so a decal wins over the wall it is on.
    polygonOffsetFactor: look.polygonOffset ? -2 : 0,
    polygonOffsetUnits: look.polygonOffset ? -4 : 0,
    side: look.doubleSided ? THREE.DoubleSide : THREE.FrontSide,
    fog: look.fog,
  };
  const m = look.unlit ? new THREE.MeshBasicMaterial(params) : new THREE.MeshStandardMaterial({ ...params, roughness: 0.9, metalness: 0 });
  if (look.blending === "multiply") {
    m.blending = THREE.MultiplyBlending;
    m.premultipliedAlpha = true;
  } else if (look.blending === "double") {
    // dst·src + src·dst: twice the picture times what is behind, so its
    // mid-grey leaves the wall as it is. (Drawn straight to the screen,
    // three blends after its sRGB encode, so the picture comes out as
    // stored and doubles as in the game.)
    m.blending = THREE.CustomBlending;
    m.blendSrc = THREE.DstColorFactor;
    m.blendDst = THREE.SrcColorFactor;
    m.blendEquation = THREE.AddEquation;
  } else if (look.blending === "add") {
    // Colour added by the picture's alpha (its mask), dimmed: the game
    // tints and dims these by shader constants not read here, and a light
    // volume is a stack of sheets that at full strength adds up to white.
    m.color.multiplyScalar(ADD_STRENGTH);
    m.blending = THREE.CustomBlending;
    m.blendSrc = THREE.SrcAlphaFactor;
    m.blendDst = THREE.OneFactor;
    m.blendEquation = THREE.AddEquation;
  }
  // A factor on what is behind, not a colour of its own.
  if (look.blending === "multiply" || look.blending === "double") m.toneMapped = false;
  m.userData = { ...src.userData };
  src.dispose();
  mesh.material = m;
  mesh.castShadow = false;
  mesh.receiveShadow = !look.unlit;
  mesh.renderOrder = look.renderOrder;
  // The occlusion pass leaves it out: it is not a surface that shades its surroundings.
  mesh.userData.noAo = true;
  return spec;
}
