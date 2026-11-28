//! The sky's cut (docs/graphics/retire-d3d-r5.md, "The sky's cut"): the
//! host of `render_bridge::native_sky::before_sky` over the device.
//!
//! - **A guest pass** ([`Glue::begin_guest`] to [`Glue::end_guest`]): every
//!   strip `native_residual_glue` admits is kept ([`record`]) with its
//!   section, shader, pass and decompression, and its view block (vertex
//!   registers 0 to 17) is checked against the device's at
//!   `0x0003ede0`'s entry. A strip kept on the guest, or a view block that
//!   differs, leaves the pass incomplete; a differing view also stops the
//!   cut's arm (`sky_cut_view`), as does a strip whose stencil is not the
//!   one the last pass's strip in its place drew with (`sky_cut_stencil`:
//!   a replay draws with its template's). A complete pass replaces the
//!   kept strips.
//! - **A cut pass** ([`Glue::draw`]): each kept strip replayed
//!   (`IndexedPass::sky_replay`) with the rows made from the sky's
//!   transform (`sky_rows::rows_of`) and its pass evaluated at the render
//!   clock, all as one `SkyCut` reserved at the boundary and committed; a
//!   refusal before the commit draws no sky that pass
//!   (`scene_dropped_sky_<why>`), the guest's pass still passed over.

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::world_layers_gpu::{IndexedPass, SkyCut, SkyTemplate};

/// One kept strip.
#[derive(Clone)]
struct Strip {
    template: SkyTemplate,
    section: u32,
    shader: u32,
    pass: u32,
    decompression: [frame_scene::Row; 3],
}

/// The strips of the last complete guest pass, and the pass recording.
#[derive(Default)]
struct Kept {
    generation: u64,
    strips: Vec<Strip>,
    recording: Option<Recording>,
}

struct Recording {
    generation: u64,
    strips: Vec<Strip>,
    complete: bool,
    view: [[f32; 4]; d3d::VIEW_CONSTANTS],
}

static KEPT: Mutex<Kept> = Mutex::new(Kept { generation: 0, strips: Vec::new(), recording: None });

/// Whether strip constants `constants` hold the device's view block `view`
/// bit for bit.
fn same_view(constants: &[[f32; 4]], view: &[[f32; 4]; d3d::VIEW_CONSTANTS]) -> bool {
    constants.get(..d3d::VIEW_CONSTANTS).is_some_and(|c| {
        c.iter().zip(view).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits))
    })
}

/// A sky strip of a guest pass: `admitted` the pass and item the strip
/// drew with, `None` when it kept its guest draw. Outside a guest pass,
/// nothing is kept.
pub(crate) fn record(
    generation: u64, admitted: Option<(&IndexedPass, &render_bridge::native_sky::SkyItem)>, draw: &d3d::ResolvedDraw,
) {
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let kept = &mut *kept;
    let Some(recording) = kept.recording.as_mut() else { return };
    if recording.generation != generation {
        recording.complete = false;
        return;
    }
    if !same_view(&draw.shader.constants[..], &recording.view) {
        if recording.complete {
            render_bridge::native_sky::count("cut_view");
            render_bridge::native_sky::cut_miss(generation);
        }
        recording.complete = false;
        return;
    }
    match admitted.and_then(|(pass, item)| Some((pass.sky_template()?, item))) {
        Some((template, item)) => {
            // A replay draws with the stencil its template drew with: a
            // strip whose stencil differs from the last pass's strip in
            // its place stops the arm for the BSP (`sky_cut_stencil`).
            let previous = (kept.generation == generation)
                .then(|| kept.strips.get(recording.strips.len()))
                .flatten()
                .map(|p| p.template.stencil());
            if !stencil_holds(previous, template.stencil()) {
                if recording.complete {
                    render_bridge::native_sky::count("cut_stencil");
                    render_bridge::native_sky::cut_miss(generation);
                }
                recording.complete = false;
                return;
            }
            recording.strips.push(Strip {
                template,
                section: item.section,
                shader: item.shader,
                pass: item.pass,
                decompression: item.decompression,
            });
        }
        None => recording.complete = false,
    }
}

/// Whether a strip's stencil `now` holds the one the last complete pass's
/// strip in its place drew with (`previous`; `None` for a strip with no
/// such place): test, ops, masks and reference alike.
fn stencil_holds(previous: Option<&gfx::state::StencilState>, now: &gfx::state::StencilState) -> bool {
    previous.is_none_or(|p| p == now)
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::backend::Backend + Send + 'static> render_bridge::native_sky::CutHost for Glue<B> {
    fn begin_guest(&self, _mem: &dyn GuestMemory) {
        let Ok(device) = self.device.lock() else { return };
        let view = std::array::from_fn(|r| device.constant(r));
        drop(device);
        let generation = render_bridge::native_map_work::bsp_generation();
        KEPT.lock().unwrap_or_else(|e| e.into_inner()).recording =
            Some(Recording { generation, strips: Vec::new(), complete: true, view });
    }

    fn end_guest(&self) {
        let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(recording) = kept.recording.take() else { return };
        if recording.complete && !recording.strips.is_empty() {
            kept.generation = recording.generation;
            kept.strips = recording.strips;
        } else {
            kept.strips.clear();
        }
    }

    fn draw(&self, mem: &mut dyn GuestMemory) -> Result<usize, &'static str> {
        let strips = {
            let kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
            if kept.generation != render_bridge::native_map_work::bsp_generation() || kept.strips.is_empty() {
                return Err("strips");
            }
            kept.strips.clone()
        };
        let frame = crate::d3d_diag::presents() + 1;
        let mut passes = Vec::with_capacity(strips.len());
        for strip in &strips {
            let rows = native_render::sky_rows::rows_of(&crate::native_world_glue::ReadMemory(&*mem), strip.section)
                .map_err(|_| "rows")?;
            let constants = crate::native_sky_glue::evaluate(&*mem, strip.shader, strip.pass).map_err(|why| match why {
                "unloaded" => "unloaded",
                _ => "constants",
            })?;
            passes.push(IndexedPass::sky_replay(&strip.template, frame, rows, strip.decompression, constants).map_err(|_| "pass")?);
        }
        let cut = SkyCut::new(passes).ok_or("pass")?;
        let ranges = cut.len();
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let mut d = self.device.lock().map_err(|_| "device")?;
        if !d.can_external_pass()
            || !d.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)).map_err(|_| "device")?
        {
            return Err("device");
        }
        let plan = d3d::ExternalPlan { stage: timeline::native::Stage::Sky, view, ranges, slots: Vec::new() };
        let reservation = d
            .prepare_external_bound(&mut crate::native_lights_glue::DeviceMemory(mem), plan, Box::new(cut), Vec::new())
            .map_err(|_| "device")?
            .ok_or("pass")?;
        reservation.commit();
        Ok(ranges)
    }
}

/// Installs the cut's host over the device (`native_residual_glue::install`).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    render_bridge::native_sky::set_cut_host(Arc::new(Glue { device: Arc::clone(device) }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view block is compared bit for bit over registers 0 to 17 only.
    #[test]
    fn a_strip_holds_the_view_block_bit_for_bit() {
        let view = std::array::from_fn(|r| [r as f32, 1.0, 2.0, 3.0]);
        let mut constants = vec![[0.0f32; 4]; 192];
        constants[..d3d::VIEW_CONSTANTS].copy_from_slice(&view);
        assert!(same_view(&constants, &view));
        constants[40] = [9.0; 4];
        assert!(same_view(&constants, &view), "registers past the block are the strip's");
        assert_eq!(view[0][0].to_bits(), 0.0f32.to_bits());
        constants[0][0] = -0.0;
        assert!(!same_view(&constants, &view), "bit for bit: -0 is not 0");
        assert!(!same_view(&constants[..4], &view));
    }

    /// A strip with no place in the last pass holds; one in a place holds
    /// only the stencil drawn there, its reference included.
    #[test]
    fn a_strip_holds_the_stencil_its_place_drew_with() {
        let mark = native_render::object_state::structure_stencil(native_render::object_state::STENCIL_MARK_REFERENCE);
        let off = gfx::state::StencilState { enable: false, ..mark };
        assert!(stencil_holds(None, &off));
        assert!(stencil_holds(Some(&mark), &mark));
        assert!(stencil_holds(Some(&off), &off));
        assert!(!stencil_holds(Some(&mark), &off));
        let other = gfx::state::StencilState { reference: mark.reference.wrapping_add(1), ..mark };
        assert!(!stencil_holds(Some(&mark), &other));
    }
}
