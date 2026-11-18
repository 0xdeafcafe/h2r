// The glTF loader every viewer model goes through. The extracted art's glbs
// carry their vertex and index data compressed with EXT_meshopt_compression
// (docs/live/live-art.md, "Packing"), which a loader without a meshopt
// decoder refuses; the decoder is meshoptimizer's own WebAssembly, decoding
// far faster than the bytes arrive. This file needs three.js, so it is only
// ever loaded lazily.

import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { MeshoptDecoder } from "three/examples/jsm/libs/meshopt_decoder.module.js";

/** A GLTFLoader that reads meshopt-compressed glbs. */
export function gltfLoader(): GLTFLoader {
  return new GLTFLoader().setMeshoptDecoder(MeshoptDecoder);
}
