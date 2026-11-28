#![cfg(feature = "gpu")]

use std::sync::Mutex;

use gfx::D3DFormat;

// Frame counters are process-wide.
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

const SIZE: u32 = 32;
const FORMAT: D3DFormat = D3DFormat::LinA8R8G8B8;
const DEPTH: D3DFormat = D3DFormat::LinD24S8;

#[path = "world_layers_gpu/frame_graph.rs"]
mod frame_graph;

#[path = "world_layers_gpu/sun_tiles.rs"]
mod sun_tiles;

#[path = "world_layers_gpu/cone_shadows.rs"]
mod cone_shadows;
