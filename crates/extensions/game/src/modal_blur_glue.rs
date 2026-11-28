//! The blur under a dialog drawn from the overlay
//! (`render_bridge::native_hud::modal_blur`; docs/graphics/native-post-hud.md,
//! "The blur under a dialog"): at the wrapper's entry the overlay's `Blur`
//! item is placed as a stage over the bound target (target 0, the UI's),
//! drawn with the call's steps (the copy back after an odd count of steps
//! is one more of them). A quad that names a target other than 0 or 1 is
//! not planned and stops by name.

use std::sync::Mutex;

use engine::memory::GuestMemory;
use frame_scene::{ClipRect, Overlay, OverlayBlur};
use frame_types::Slot;
use native_render::overlay::{NoGlyphs, OverlayPlan};
use native_render::program::ViewInputs;
use native_render::ui_gpu::{NoTextures, OverlayGpu};
use native_render::{Stage, StageWork};
use render_bridge::native_hud::modal_blur::ModalBlur;

/// The slot of a quad into `target`: target 0 is the one bound when the
/// call starts (the UI's), target 1 the blur's scratch.
fn slot(target: u32) -> Option<u8> {
    match target {
        0 => Some(0),
        1 => Some(1),
        _ => None,
    }
}

/// The game's quads, `(source, dest)` in order: the blur's draws, then
/// after an odd count the copy back to target 0 (`0x000351a0`, one more
/// quad of `0x00034a90`).
fn quads(call: &ModalBlur) -> impl Iterator<Item = (u32, u32)> + '_ {
    call.draws.iter().map(|d| (d.source, d.dest)).chain(call.copy)
}

/// Each quad's slot, in order; `None` when one names another target.
pub fn targets(call: &ModalBlur) -> Option<Vec<u8>> {
    quads(call).map(|(_, dest)| slot(dest)).collect()
}

/// Each quad's source slot, in order.
fn sources(call: &ModalBlur) -> Option<Vec<u8>> {
    quads(call).map(|(source, _)| slot(source)).collect()
}

/// Places `call`'s blur at the wrapper's entry: whether it did.
pub fn arm(mem: &mut dyn GuestMemory, call: &ModalBlur) -> bool {
    let (Some(_), Some(sources)) = (targets(call), sources(call)) else {
        return false;
    };
    crate::native_targets_glue::push(&*mem, Slot::Overlay, ModalBlurPass { blur: call.blur.clone(), ranges: sources.len() });
    true
}

/// The wrapper's return (`esp` its return slot): the blur was placed at
/// its entry.
pub fn finish(_: &mut dyn GuestMemory, _: u32) -> bool {
    true
}

/// The overlay's programs on the device last drawn with.
static RENDERER: Mutex<Option<(wgpu::Device, OverlayGpu)>> = Mutex::new(None);

/// The blur drawn into the bound target where the game's quads were.
struct ModalBlurPass {
    blur: OverlayBlur,
    /// The game's quads it stands for.
    ranges: usize,
}

/// `blur` over the whole of a target of `guest` pixels: one `Blur` item.
fn overlay(blur: &OverlayBlur, guest: (u32, u32)) -> Option<Overlay> {
    let size = [guest.0, guest.1];
    let mut overlay = Overlay::new(size);
    overlay.push_blur(ClipRect::full(size), blur.clone()).ok()?;
    Some(overlay)
}

impl Stage for ModalBlurPass {
    fn name(&self) -> &'static str {
        "dialog blur"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &ViewInputs) -> StageWork {
        let extent = t.color.texture().size();
        let overlay = overlay(&self.blur, view.guest_size).unwrap_or_else(|| unimplemented!("dialog blur: the blur item"));
        let plan = OverlayPlan::new(&overlay, [extent.width, extent.height], &NoGlyphs);
        let mut lock = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        if lock.as_ref().is_none_or(|(d, _)| *d != t.device) {
            *lock = Some((t.device.clone(), OverlayGpu::new(&t.device, &t.queue)));
        }
        let Some((_, gpu)) = lock.as_mut() else { unreachable!() };
        let (encoded, stats) =
            gpu.prepare(&t.device, &t.color, &plan, &NoTextures).unwrap_or_else(|why| unimplemented!("dialog blur: {why}"));
        if stats.blurs != 1 || stats.refused_blur != 0 || stats.refused_clip != 0 {
            unimplemented!("dialog blur: the target cannot be copied or sampled");
        }
        native_render::ui_gpu::count_overlay(&stats);
        StageWork::Encoded(gfx::external::ExternalEncoded { ranges: self.ranges, ..encoded })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::glow::{BlurDraw, Psdef};

    fn call(ends: &[(u32, u32)]) -> ModalBlur {
        ModalBlur {
            wrapper_esp: 0x0010_002c,
            caller: 0x0023_512d,
            draws: ends
                .iter()
                .map(|&(source, dest)| BlurDraw {
                    source,
                    dest,
                    spread: 1.0f32.to_bits(),
                })
                .collect(),
            copy: None,
            psdef: Psdef([0; engine::glow::PSDEF_WORDS]),
            blur: OverlayBlur {
                tap: 0.5,
                spreads: vec![1.0; ends.len()],
                pick: 0.0,
            },
        }
    }

    /// Quads into target 0 take the bound slot; quads into target 1 the
    /// slot their first quad pins. Any other target is not planned.
    #[test]
    fn each_quad_takes_its_targets_slot() {
        assert_eq!(
            targets(&call(&[(0, 1), (1, 0), (0, 1), (1, 0)])),
            Some(vec![1, 0, 1, 0])
        );
        assert_eq!(targets(&call(&[(0, 1), (1, 7)])), None);
        // An odd count: the copy back is one more quad, into target 0
        // from target 1.
        let mut odd = call(&[(0, 1), (1, 0), (0, 1)]);
        odd.copy = Some((1, 0));
        assert_eq!(targets(&odd), Some(vec![1, 0, 1, 0]));
        assert_eq!(sources(&odd), Some(vec![0, 1, 0, 1]));
    }

    /// The pass draws one blur item over the target's guest size, with the
    /// call's steps and combiner.
    #[test]
    fn the_pass_draws_the_calls_blur_over_the_whole_target() {
        let mut c = call(&[(0, 1), (1, 0)]);
        c.blur.pick = 0.25;
        let o = overlay(&c.blur, (640, 480)).unwrap();
        assert_eq!(o.size, [640, 480]);
        assert_eq!(o.items.len(), 1);
        assert_eq!(o.items[0].clip, ClipRect::full([640, 480]));
        assert_eq!(o.blur_of(&o.items[0]), Some(&c.blur));
        for target in [
            [1280, 960],
            [1920, 1080],
            [2520, 1080],
            [5120, 2880],
            [5120, 2648],
        ] {
            let plan = OverlayPlan::new(&o, target, &NoGlyphs);
            assert_eq!(plan.blurs.len(), 1);
            let blur = &plan.blurs[0];
            assert_eq!(blur.pick, 0.25);
            let clip = blur.scissor;
            assert_eq!(
                [clip.x, clip.y, clip.width, clip.height],
                [0, 0, target[0], target[1]]
            );
            assert_eq!(
                blur.offsets[0],
                [0.5 / 640.0, 0.5 / 480.0],
                "tap offsets are expressed as fractions of the guest frame"
            );
        }
    }
}
