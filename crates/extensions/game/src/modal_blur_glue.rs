//! The blur under a dialog drawn from the overlay
//! (`render_bridge::native_hud::modal_blur`; docs/graphics/native-post-hud.md,
//! "The blur under a dialog"). The game's code runs whole; the device
//! leaves out its quads and, at the wrapper's return, draws the overlay's
//! `Blur` item into target 0 in their place (the copy back after an odd
//! count of steps is one more of them). A quad off the plan, a quad
//! whose state is not the plain one the blur step draws, or a pass that
//! does not record gives the game's quads back in order.

use std::sync::Mutex;

use d3d::backend::{ExternalAttachments, ExternalEncoded, ExternalPassRequest};
use engine::memory::GuestMemory;
use frame_scene::{ClipRect, Overlay, OverlayBlur};
use gfx::vertex::PrimitiveType;
use native_render::overlay::{NoGlyphs, OverlayPlan};
use native_render::ui_gpu::{NoTextures, OverlayGpu};
use render_bridge::native_hud::modal_blur::ModalBlur;

use crate::native_lights_glue::DeviceMemory;

/// The swallow's slot of a quad into `target`: target 0 is the one bound
/// when the call starts (the UI's), target 1 is pinned by its first quad.
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
    call.draws
        .iter()
        .map(|d| (d.source, d.dest))
        .chain(call.copy)
}

/// Each quad's slot, in order; `None` when one names another target.
pub fn targets(call: &ModalBlur) -> Option<Vec<u8>> {
    quads(call).map(|(_, dest)| slot(dest)).collect()
}

/// Each quad's source slot, in order.
fn sources(call: &ModalBlur) -> Option<Vec<u8>> {
    quads(call).map(|(source, _)| slot(source)).collect()
}

/// Arms the device to leave out `call`'s quads and draw its blur at the
/// wrapper's return: whether it did.
pub fn arm<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    call: &ModalBlur,
) -> bool {
    let (Some(targets), Some(sources)) = (targets(call), sources(call)) else {
        return false;
    };
    let Ok(mut device) = device.lock() else {
        return false;
    };
    let swallow = d3d::Swallow {
        call: Some(d3d::SwallowCall {
            entry: engine::points::UI_MODAL_BLUR.base,
            esp: call.wrapper_esp,
            ret: call.caller,
        }),
        end_returns: Vec::new(),
        headers: vec![None, None],
        targets,
        primitive: PrimitiveType::TriangleFan,
        vertices: 4,
        shapes: Vec::new(),
        drop: true,
        pass: Box::new(ModalBlurPass {
            blur: call.blur.clone(),
            sources,
        }),
    };
    matches!(device.swallow(&mut DeviceMemory(mem), swallow), Ok(true))
}

/// The wrapper's return (`esp` its return slot): whether the armed pass
/// replaced the game's quads.
pub fn finish<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    esp: u32,
) -> bool {
    let Ok(mut device) = device.lock() else {
        return false;
    };
    let before = device.stats().swallow_commits;
    let finished = device
        .finish_swallow(
            &mut DeviceMemory(mem),
            engine::points::UI_MODAL_BLUR.base,
            esp,
        )
        .is_ok();
    finished && device.stats().swallow_commits > before
}

/// The overlay's programs on the device last drawn with.
static RENDERER: Mutex<Option<(wgpu::Device, OverlayGpu)>> = Mutex::new(None);

/// The blur drawn into target 0 (slot 0) where the game's quads were.
struct ModalBlurPass {
    blur: OverlayBlur,
    /// Each quad's source slot, in order.
    sources: Vec<u8>,
}

/// `blur` over the whole of a target of `guest` pixels: one `Blur` item.
fn overlay(blur: &OverlayBlur, guest: (u32, u32)) -> Option<Overlay> {
    let size = [guest.0, guest.1];
    let mut overlay = Overlay::new(size);
    overlay.push_blur(ClipRect::full(size), blur.clone()).ok()?;
    Some(overlay)
}

impl ModalBlurPass {
    fn prepare(&self, t: &ExternalAttachments) -> Result<ExternalEncoded, &'static str> {
        let slot = t
            .slots
            .first()
            .and_then(Option::as_ref)
            .ok_or("dialog blur target is unavailable")?;
        let texture = slot.attachment.texture();
        let extent = texture.size();
        if (extent.width, extent.height) != slot.size {
            return Err("dialog blur target size");
        }
        let overlay = overlay(&self.blur, slot.guest_size).ok_or("dialog blur refused")?;
        let plan = OverlayPlan::new(&overlay, [extent.width, extent.height], &NoGlyphs);
        let mut lock = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        if lock.as_ref().is_none_or(|(d, _)| *d != t.device) {
            *lock = Some((t.device.clone(), OverlayGpu::new(&t.device, &t.queue)));
        }
        let (_, gpu) = lock.as_mut().ok_or("dialog blur device")?;
        let (encoded, stats) = gpu.prepare(&t.device, &slot.attachment, &plan, &NoTextures)?;
        // A target the GPU cannot copy or sample refuses the blur: the
        // game's quads are given back.
        if stats.blurs != 1 || stats.refused_blur != 0 || stats.refused_clip != 0 {
            return Err("dialog blur target cannot be copied or sampled");
        }
        if t.copy == d3d::TargetCopy::Display {
            native_render::ui_gpu::count_overlay(&stats);
        }
        Ok(ExternalEncoded {
            ranges: self.sources.len(),
            draws: encoded.draws,
            passes: encoded.passes,
            commands: encoded.commands,
        })
    }
}

impl d3d::ExternalPass for ModalBlurPass {
    fn colour_only(&self) -> bool {
        true
    }

    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: true,
            depth_stencil: false,
        }
    }

    fn record_encoded(
        &mut self,
        t: &ExternalAttachments,
        _: ExternalPassRequest,
    ) -> Option<Result<ExternalEncoded, &'static str>> {
        Some(self.prepare(t))
    }
}

impl d3d::SwallowPass for ModalBlurPass {
    /// Each game quad draws unblended into every channel and samples the
    /// target its plan names, whole: the plain state the overlay's blur
    /// step draws. Targets 0 and 1 are the same size.
    fn validate(
        &self,
        originals: &[d3d::SwallowOriginal<'_>],
        slots: &[Option<d3d::SwallowTarget>],
    ) -> Result<(), String> {
        validate_slots(slots)?;
        if originals.len() != self.sources.len() {
            return Err("dialog blur quad count differs".into());
        }
        for (original, &source) in originals.iter().zip(&self.sources) {
            if !crate::native_post_glue::matches_colour_state(
                original.draw,
                None,
                wgpu::ColorWrites::ALL,
                None,
            ) {
                return Err("dialog blur colour state differs".into());
            }
            // The UI's target may have no alpha (X8R8G8B8): the blur's
            // colour reads none, and its alpha is not shown.
            for stage in 0..4 {
                if original.sampled[stage] {
                    crate::native_post_glue::check_bound_source(
                        original,
                        stage,
                        Some(source),
                        slots,
                        true,
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Targets 0 and 1 both live, and the same size.
fn validate_slots(slots: &[Option<d3d::SwallowTarget>]) -> Result<(), String> {
    match slots {
        [Some(zero), Some(one)] if zero.size == one.size => Ok(()),
        [Some(_), Some(_)] => Err("dialog blur targets differ in size".into()),
        _ => Err("dialog blur targets are not both live".into()),
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

    /// Both targets must be live and the same size.
    #[test]
    fn the_targets_must_match() {
        let t = |w| {
            Some(d3d::SwallowTarget {
                data: 0x1000,
                size: (w, 480),
            })
        };
        assert!(validate_slots(&[t(640), t(640)]).is_ok());
        assert!(
            validate_slots(&[t(640), t(320)])
                .unwrap_err()
                .contains("size")
        );
        assert!(validate_slots(&[t(640), None]).is_err());
        assert!(validate_slots(&[t(640)]).is_err());
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
