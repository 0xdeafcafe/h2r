//! The ocean's reflection `0x0003bd00(height, colour)` (Code,
//! `0x0003bd00..0x0003be97`; docs/graphics/retire-d3d-display.md, "The
//! water end"), in Rust: the sky drawn into `Game(0x15)` from the camera
//! mirrored about the water's height. It calls no guest helper.
//!
//! | Guest | Made |
//! |---|---|
//! | `[0x004670bc] = 1`, the draw state's reset `0x00016b10` | the open shader's restore list through [`Host::restore`], then `frame_step::layer::clear_cache` |
//! | the context's frustum `0x00485618` (`0x74` bytes) and camera `0x0048568c` (`0xc0`) copied | read once; written back at the end as the guest does |
//! | `[0x004858b8] = 0x15`; `0x00014bc0(al 0, 0x15, 0)`: target `0x15`'s surface (`0x0001dcf0`), no depth, flag 0, the viewport the surface's size | [`native_targets::cut_select`], [`Host::viewport`] over [`Host::surface_size`] |
//! | `D3DDevice_Clear(0, 0, 0xf0, colour, 1.0, 0)` | [`Host::clear`] |
//! | the camera mirrored: its basis's third axis (`+0x1c..+0x24`) negated, its point's height (`+0x30`) = height × `[0x0045dc0c]` − `+0x30`; the inverse at `+0x34` made again (`0x00141590(ecx = camera, eax = camera + 0x34)`); the frustum's window `+0x30` set to 0, 0, 256, 512 | [`mirrored`], `engine::light_frusta::inverse_words` |
//! | `0x00012fa0(frustum, camera, 0, 0x15)` | `view_setup::transform` over the view staged below the stack, its shadow, [`Host::constants`] |
//! | the sky `0x0003ede0` | [`Device::sky`]: the native sky's cut replayed on the bound target |
//! | `0x00014bc0(al 1, the view's target, 0)` | the view's target (0 or `0x12`) bound again with its depth and viewport (`view_setup::viewport`) |
//! | `[0x004858b8]` back; `0x00012fa0(frustum, camera, 0, target)` | the view's constants made again from the saved copies |
//!
//! The check `[0x00485607]` with `[0x004858b8]` 0 (`0x00014ac0`) never
//! passes: the target was just set to `0x15`. What it cannot make is a
//! cause, `water ocean copy: <cause>`: `reflection` (target `0x15` has no
//! live surface or size), `bind`, `viewport`, `clear`, `constants` (the
//! device refused it), `sky` (the native sky drew nothing), `view_target`
//! (the view's target is not 0 or `0x12`), `cache` and `restore` (the
//! draw state's reset).

use guest_layout::memory::{GuestMemory, MemFault};

use super::{Cause, Device};
use crate::native_targets::{self, Host};
use crate::view_setup;

/// The target the reflection is drawn into.
pub const REFLECTION: u32 = 0x15;
/// The context's frustum and camera (`view_setup::CONTEXT + 0x18`,
/// `+ 0x8c`) and their sizes.
pub const FRUSTUM_OFFSET: u32 = 0x18;
pub const CAMERA_OFFSET: u32 = 0x8c;
pub const FRUSTUM_BYTES: u32 = 0x74;
pub const CAMERA_WORDS: usize = 0x30;
/// The window the copy writes at the frustum's `+0x30`: top, left,
/// bottom, right.
pub const WINDOW: [u16; 4] = [0, 0, 0x100, 0x200];
/// The definition's height and reflection colour (`0x0003c4e6..0x0003c4f9`).
pub const HEIGHT: u32 = 0x84;
pub const COLOUR: u32 = 0x58;
/// The D3D clear's flags: the four colour channels, no depth or stencil.
pub const CLEAR_FLAGS: u32 = 0xf0;
/// The multiplier of the height, `[0x0045dc0c]`, and the guest's 1.0.
const TWICE: u32 = 0x0045_dc0c;
const ONE: u32 = 0x0045_dbc0;
/// Bytes below [`Device::scratch`] the pass stages its view, its
/// constants and its viewport in (the guest's own frame is `0x284`
/// bytes deep).
pub const FRAME: u32 = 0x5b0;
const STAGED_CONSTANTS: u32 = 0x2a0;
const STAGED_VIEWPORT: u32 = 0x360;

fn unread(_: MemFault) -> Cause {
    "unread"
}

fn unwritable(_: MemFault) -> Cause {
    "unwritable"
}

/// The camera `0x0003bd00` hands `0x00012fa0`: its basis's third axis
/// (words 7 to 9) negated, its point's third word (12) `height × twice −
/// z`, then the inverse at `+0x34` (words 13 to 25) made again from the
/// basis. The rest is the camera's.
pub fn mirrored(camera: &[u32; CAMERA_WORDS], height: f32, twice: f32, one: f32) -> [u32; CAMERA_WORDS] {
    let mut out = *camera;
    for w in &mut out[7..10] {
        *w = (0.0 - f32::from_bits(*w)).to_bits();
    }
    out[12] = (height * twice - f32::from_bits(out[12])).to_bits();
    let rows: [u32; 13] = std::array::from_fn(|i| out[i]);
    out[13..26].copy_from_slice(&engine::light_frusta::inverse_words(&rows, one));
    out
}

/// The context's view as `0x00012fa0` reads it, with its kind word
/// `target` and its wide byte clear (the call's stack arguments), the
/// frustum and camera given.
fn staged_view(context: &[u8], frustum: &[u8], camera: &[u32; CAMERA_WORDS], target: u32) -> Vec<u8> {
    let mut view = context.to_vec();
    view[2..4].copy_from_slice(&(target as u16).to_le_bytes());
    view[7] = 0;
    let f = FRUSTUM_OFFSET as usize;
    view[f..f + frustum.len()].copy_from_slice(frustum);
    let c = CAMERA_OFFSET as usize;
    for (k, w) in camera.iter().enumerate() {
        view[c + 4 * k..c + 4 * k + 4].copy_from_slice(&w.to_le_bytes());
    }
    view
}

/// `0x00012fa0` over a staged view: its shadow unless bit 4 of the
/// constant flags is set, then the 48 words to the device.
fn upload(mem: &mut dyn GuestMemory, host: &dyn Host, base: u32, view: &[u8]) -> Result<(), Cause> {
    mem.write(base, view).map_err(unwritable)?;
    let words = view_setup::transform(&*mem, base)?;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    if mem.u8(view_setup::CONSTANT_FLAGS).map_err(unread)? & 0x10 == 0 {
        mem.write(view_setup::TRANSFORM_SHADOW, &bytes).map_err(unwritable)?;
    }
    let at = base.wrapping_add(STAGED_CONSTANTS);
    mem.write(at, &bytes).map_err(unwritable)?;
    if host.constants(mem, 0, at, 48) { Ok(()) } else { Err("copy_constants") }
}

/// The viewport words staged at `at`, then set.
fn viewport(mem: &mut dyn GuestMemory, host: &dyn Host, at: u32, words: &[u32; 6]) -> Result<(), Cause> {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    mem.write(at, &bytes).map_err(unwritable)?;
    if host.viewport(mem, at) { Ok(()) } else { Err("copy_viewport") }
}

/// The draw state's reset `0x00016b10`: the open shader's restore list,
/// then the cache cleared.
pub fn reset_draw_state(mem: &mut dyn GuestMemory, host: &dyn Host) -> Result<(), Cause> {
    let open = crate::open_shader::read(&*mem).map_err(|_| "copy_cache")?;
    if let Some(open) = &open
        && !host.restore(mem, &open.writes)
    {
        return Err("copy_restore");
    }
    crate::frame_step::layer::clear_cache(mem).map_err(unwritable)
}

/// `0x0003bd00` for `definition`: its height and colour read as the body
/// pushes them.
pub fn pass(mem: &mut dyn GuestMemory, device: &mut dyn Device, definition: u32) -> Result<(), Cause> {
    let height = mem.f32(definition.wrapping_add(HEIGHT)).map_err(unread)?;
    let colour = view_setup::pack(&*mem, definition.wrapping_add(COLOUR))?;
    let base = device.scratch().wrapping_sub(FRAME);
    mem.set_u8(engine::render_loop::layer::DRAW_FLAG, 1).map_err(unwritable)?;
    reset_draw_state(mem, device.host())?;
    let context = mem.bytes(view_setup::CONTEXT, view_setup::VIEW_SIZE as usize).map_err(unread)?;
    let f = FRUSTUM_OFFSET as usize;
    let frustum = context[f..f + FRUSTUM_BYTES as usize].to_vec();
    let c = CAMERA_OFFSET as usize;
    let camera: [u32; CAMERA_WORDS] =
        std::array::from_fn(|k| u32::from_le_bytes(context[c + 4 * k..c + 4 * k + 4].try_into().unwrap()));
    let view_target = mem.u32(view_setup::TARGET).map_err(unread)?;
    let one = mem.f32(ONE).map_err(unread)?;

    // Target 0x15, no depth, its whole surface; cleared to the colour.
    mem.set_u32(view_setup::TARGET, REFLECTION).map_err(unwritable)?;
    let host = device.host();
    let surface = view_setup::cached(&*mem, REFLECTION)?;
    if surface == 0 {
        return Err("copy_reflection");
    }
    native_targets::cut_select(mem, host, surface, 0, 0).map_err(|cause| if cause == "device" { "copy_bind" } else { cause })?;
    let (width, height_px) = host.surface_size(&*mem, surface).ok_or("copy_reflection")?;
    let at = base.wrapping_add(STAGED_VIEWPORT);
    viewport(mem, host, at, &[0, 0, width, height_px, 0, one.to_bits()])?;
    if !host.clear(mem, CLEAR_FLAGS, colour, 1f32.to_bits(), 0) {
        return Err("copy_clear");
    }

    // The mirrored view, then the sky.
    let twice = mem.f32(TWICE).map_err(unread)?;
    let mut window = frustum.clone();
    for (k, v) in WINDOW.iter().enumerate() {
        window[0x30 + 2 * k..0x32 + 2 * k].copy_from_slice(&v.to_le_bytes());
    }
    let mirror = mirrored(&camera, height, twice, one);
    upload(mem, host, base, &staged_view(&context, &window, &mirror, REFLECTION))?;
    device.sky(mem)?;

    // The view's target again, then its constants from the saved copies.
    let host = device.host();
    let target = match view_target {
        0 | view_setup::CACHED_TARGET => view_target,
        _ => return Err("copy_view_target"),
    };
    let depth = mem.u32(view_setup::VIEW_DEPTH).map_err(unread)?;
    let (colour_surface, flag) = if target == 0 {
        (mem.u32(view_setup::VIEW_COLOUR).map_err(unread)?, 1)
    } else {
        (view_setup::cached(&*mem, target)?, 0)
    };
    native_targets::cut_select(mem, host, colour_surface, depth, flag).map_err(|cause| if cause == "device" { "copy_bind" } else { cause })?;
    let words = view_setup::viewport(&*mem, view_setup::CONTEXT, target)?;
    viewport(mem, host, at, &words)?;
    mem.write(view_setup::CONTEXT + FRUSTUM_OFFSET, &frustum).map_err(unwritable)?;
    mem.write(view_setup::CONTEXT + CAMERA_OFFSET, &context[c..c + 4 * CAMERA_WORDS]).map_err(unwritable)?;
    mem.set_u32(view_setup::TARGET, view_target).map_err(unwritable)?;
    upload(mem, host, base, &staged_view(&context, &frustum, &camera, view_target))
}
