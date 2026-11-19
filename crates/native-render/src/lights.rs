//! Stage 4, dynamic lights and shadows: what the native renderer takes of
//! one dynamic light, and the runs it draws lights in
//! (docs/graphics/native-renderer.md, "Stage 4: lights and shadows").
//!
//! Research points (Code, `h2 disasm`):
//!
//! - `0x00031c80` (`render_dynamic_lights`, from `render_scene_passes` at
//!   `0x0002c03e`, after layer 7 and before `0x0002c370(1)` and layer 2)
//!   builds the visible-light list (`0x00031910`, entries of `0x14` bytes:
//!   `+0` the light datum, `+8` a weight), then per light above the
//!   weight threshold: re-poses an attached light (`0x00031dac`), records
//!   it for the first-person pass (`0x004c6b00`, at most eight), decides
//!   the shadow (`0x00031e02`), gathers what it touches into the light's
//!   list (`0x000c1d90`, `0x001320f0`, `0x00031660`, then the objects'
//!   render records `0x00133720`), and calls `0x00031170` once.
//! - `0x00031170` (`render_light_passes`, its only caller `0x00031f26`):
//!   EAX the `ligh` datum, ECX a context from `0x000c17f0`, stack `(light
//!   datum, &position, shadow byte, weight)`, `ret 0x10`. It sets the
//!   per-light flags `0x004ba014` and `0x004ba018`, the light's constants
//!   (`0x0004a1e0`), the shadow projection (`0x00048ee0`, into
//!   `0x004c9798`), draws layers 10 (shadow generate, when shadowed), 11
//!   (shadow apply), 14, 12 (diffuse) and 13 (specular) on the light's
//!   list `0x004c1a60`, then turns stencil off, clears the scissor and sets
//!   the view's target back, and zeroes both flag words.
//!
//! The skip is that call's: a `before` answers `ret 0x10` for a light the
//! native renderer draws, so none of its layers is built or drawn; the
//! hook does the call's tail itself (the device side through the host,
//! then [`TAIL_ZEROES`]), so a skipped light leaves what a drawn one
//! does. The bookkeeping in
//! `0x00031c80` (the list, the gather, the records) still runs. Lights are
//! met in the visible list's order; consecutive native lights share one
//! [`Run`], one external pass the device issues at its next entry (the next
//! D3D light's first draw, or what follows the lights), so each run lands
//! between the D3D lights it sits between.

use std::sync::Mutex;

/// `render_dynamic_lights`.
pub const RENDER_DYNAMIC_LIGHTS: u32 = 0x0003_1c80;
/// `render_light_passes`: one light's layers.
pub const RENDER_LIGHT_PASSES: u32 = 0x0003_1170;
/// Its stack arguments' bytes (`ret 0x10`).
pub const LIGHT_PASSES_CLEANUP: u32 = 0x10;
/// The guest stores of its tail (`0x000314d7`, `0x000314dd`, `0x0003150b`,
/// `0x00031511`), `(address, bytes)`, each zero: a skipped light makes
/// them too, so the next reader sees what a drawn light leaves.
pub const TAIL_ZEROES: [(u32, u32); 4] = [
    (0x0048_58bc, 1),
    (0x004b_843c, 4),
    (0x004b_a018, 1),
    (0x004b_a014, 4),
];
/// The two words the tail's depth-clip restore (`0x00015720`) puts into the
/// device's programmable clip range.
pub const TAIL_CLIP: [u32; 2] = [0x0048_5adc, 0x0048_5ae0];

/// One dynamic light as the hook copied it out of the render's snapshot
/// of the frame (the record's world frame is refreshed for attached lights
/// just before `render_light_passes`), so the pass draws this frame's
/// light whatever the next tick does to the record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// The light datum.
    pub handle: u32,
    /// The `ligh` tag datum.
    pub definition: u32,
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    /// The record's two colours (`+0xd4`, `+0xe0`) and intensity (`+0xd0`).
    pub colour_a: [f32; 3],
    pub colour_b: [f32; 3],
    pub intensity: f32,
    /// The bounding sphere: centre, radius.
    pub bounds: ([f32; 3], f32),
    /// The caller's shadow decision (`0x00031e02`).
    pub shadow: bool,
    /// The visible list's weight.
    pub weight: f32,
    /// What the light's passes read, computed at the hook as
    /// `0x0004a1e0` and the flag code would (`None`: not computed; the
    /// host draws no such light).
    pub constants: Option<LightConstants>,
}

/// One light's constants as its passes read them, words as the game
/// stores them (docs/graphics/native-renderer.md, "Stage 4", "Engine
/// constants"); `engine::light_constants` and `engine::light_passes`
/// compute them without writing guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightConstants {
    /// The 36 dwords `0x0003a550` uploads to `c[31]`..`c[39]`.
    pub frame: [u32; 36],
    /// The per-light flags (`[0x004ba014]`, `engine::light_passes::flag`).
    pub flags: u32,
    /// Engine vectors 5 and 6: colours A and B as the frame holds them.
    pub colour_a: [u32; 3],
    pub colour_b: [u32; 3],
    /// The colour scale mode (`[0x00485a2c]`, integers 0 and 1).
    pub mode: u32,
    /// The definition's first dword (its first byte picks layer 12's
    /// implementation 4; bit 1 is integer 7), and its words `+0x8a` and
    /// `+0x80` (integers 2 and 3).
    pub definition_flags: u32,
    pub definition_8a: u16,
    pub definition_80: u16,
    /// Falloff word 0 (`[0x00485a30]`, scalar 16).
    pub falloff_0: u32,
    /// The `bitm` datum engine texture 10 (the gel) binds, image 0
    /// (`engine::light_passes::gel`). A shader-driven gel uses the live
    /// target-13 binding supplied separately by its producer.
    pub gel: Option<u32>,
    /// The constants setup also renders a shader into target 13.
    pub animated_gel: bool,
}

impl LightConstants {
    /// Whether the light casts its shadow (flag bit 2): the shadow maps
    /// are not drawn natively, so the host leaves such a light to D3D.
    pub fn shadowed(&self) -> bool {
        self.flags & 4 != 0
    }
}

/// Consecutive native lights in the game's order: one external pass. The
/// feature adds lights while the pass waits in the device; the pass takes
/// them when the device issues it ([`Run::seal`]), after which the next
/// native light needs a new run (a D3D light's draws came between).
#[derive(Debug)]
pub struct Run<T = Light> {
    inner: Mutex<(Vec<T>, bool)>,
}

impl<T> Default for Run<T> {
    fn default() -> Self {
        Self { inner: Mutex::default() }
    }
}

impl<T> Run<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `light`; `false` once the run is sealed.
    pub fn push(&self, light: T) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.1 {
            return false;
        }
        g.0.push(light);
        true
    }

    /// Takes the lights and closes the run (the device issued its pass).
    pub fn seal(&self) -> Vec<T> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.1 = true;
        std::mem::take(&mut g.0)
    }

    pub fn is_sealed(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).1
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(handle: u32) -> Light {
        Light {
            handle,
            definition: 0xe000_0010,
            position: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            colour_a: [1.0; 3],
            colour_b: [0.5; 3],
            intensity: 1.0,
            bounds: ([1.0, 2.0, 3.0], 4.0),
            shadow: false,
            weight: 0.5,
            constants: None,
        }
    }

    #[test]
    fn a_run_keeps_its_lights_in_order_until_sealed() {
        let run = Run::new();
        assert!(run.push(light(1)) && run.push(light(2)));
        assert_eq!(run.len(), 2);
        let lights = run.seal();
        assert_eq!(lights.iter().map(|l| l.handle).collect::<Vec<_>>(), vec![1, 2]);
        assert!(run.is_sealed() && run.is_empty());
        assert!(!run.push(light(3)), "a sealed run takes no more lights");
    }
}
