//! The `native-post` host (docs/graphics/native-renderer.md, stage 5):
//! the device side of a skipped `render_screen_effects`.
//!
//! Inert unless `impl.render.native_post` is on: the hook is not placed
//! otherwise. It takes a call only when every piece the call needs is in
//! place ([`refusal`]); each reason it refuses for is logged once. Today
//! the glow is not drawn natively yet, so every call that draws keeps its
//! D3D passes. What a call it takes needs is here already:
//!
//! - **The prologue's render states**: the restore list of the shader
//!   draw-state reset, set through `d3d::Device::set_render_states_inline`
//!   over the hook's guest memory, as `0x00018900` sets them (d3d's
//!   `inline_states` test runs the two side by side);
//! - **The glow's numbers**: `engine::colour_pass` and `engine::glow`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use engine::memory::GuestMemory;
use feature::native_post::Host;
use native_render::post::PostFrame;

use crate::native_lights_glue::DeviceMemory;

/// Why a call keeps its D3D passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `0x0002df00` is not drawn natively.
    LatePass,
    /// A screen effect (`0x0002d140`) may draw; none is native.
    ScreenEffects,
    /// The glow (`0x000214f0`) is not drawn natively yet.
    Glow,
    /// The pass's leftovers (texture-stage words and dirty bits, the
    /// combiner block, render states, the target) are not redone yet.
    GlowState,
    /// The call draws nothing, and its skip is not proved yet against a
    /// frame A/B.
    NotProved,
}

impl Refusal {
    const ALL: [Refusal; 5] = [
        Refusal::LatePass,
        Refusal::ScreenEffects,
        Refusal::Glow,
        Refusal::GlowState,
        Refusal::NotProved,
    ];
    fn bit(self) -> u32 {
        1 << Self::ALL.iter().position(|&r| r == self).unwrap_or(0)
    }
    fn text(self) -> &'static str {
        match self {
            Refusal::LatePass => "the late pass 0x2df00 is not native",
            Refusal::ScreenEffects => "a screen effect may draw (0x2d140 is not native)",
            Refusal::Glow => "the glow 0x214f0 is not drawn natively yet",
            Refusal::GlowState => "the glow's leftovers (stage words, combiner block, states, target) are not redone yet",
            Refusal::NotProved => "a call drawing nothing is not skipped until the frame A/B passes",
        }
    }
}

/// The first reason `frame` keeps its D3D passes, or `None` when every
/// piece is in place.
pub fn refusal(frame: &PostFrame) -> Option<Refusal> {
    if frame.late {
        return Some(Refusal::LatePass);
    }
    if frame.screen_effects_gated && frame.screen_effects > 0 {
        return Some(Refusal::ScreenEffects);
    }
    if frame.colour {
        // Both are lifted together once the native glow and its
        // leftovers land; GlowState is reported after Glow.
        return Some(Refusal::Glow);
    }
    Some(Refusal::NotProved)
}

/// Reasons already logged (bits of [`Refusal::bit`]).
static LOGGED: AtomicU32 = AtomicU32::new(0);

/// Logs `r` the first time it is seen.
fn log_once(r: Refusal) {
    if LOGGED.fetch_or(r.bit(), Ordering::Relaxed) & r.bit() == 0 {
        eprintln!("native-post: a call keeps its D3D passes: {} (logged once)", r.text());
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn accepts(&self, _mem: &dyn GuestMemory, frame: &PostFrame) -> bool {
        match refusal(frame) {
            Some(r) => {
                log_once(r);
                false
            }
            None => true,
        }
    }

    fn prologue(&self, mem: &mut dyn GuestMemory, states: &[(u32, u32)]) -> bool {
        let Ok(mut d) = self.device.lock() else { return false };
        d.set_render_states_inline(&mut DeviceMemory(mem), states).unwrap_or(false)
    }

    fn open(&self, _frame: &PostFrame) -> bool {
        false
    }
}

/// Installs the host over `device`.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    feature::native_post::set_host(Arc::new(Glue {
        device: Arc::clone(device),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(colour: bool, late: bool, effects: u32, gated: bool) -> PostFrame {
        PostFrame {
            colour,
            late,
            view_kind: 1,
            screen_effects: effects,
            screen_effects_gated: gated,
        }
    }

    #[test]
    fn every_call_has_a_reason_until_the_glow_is_native() {
        assert_eq!(refusal(&frame(true, true, 0, false)), Some(Refusal::LatePass));
        assert_eq!(refusal(&frame(true, false, 1, true)), Some(Refusal::ScreenEffects));
        assert_eq!(refusal(&frame(true, false, 1, false)), Some(Refusal::Glow));
        assert_eq!(refusal(&frame(false, false, 0, true)), Some(Refusal::NotProved));
        let bits: Vec<u32> = Refusal::ALL.iter().map(|r| r.bit()).collect();
        assert_eq!(bits, vec![1, 2, 4, 8, 16], "one bit each");
    }
}
