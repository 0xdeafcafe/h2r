//! The light passes `0x00031170`, per light, as steps
//! (docs/graphics/framescene.md, slice 3). **Code**, `h2 disasm`: after
//! the flag code sets `[0x004ba014]` ([`SHADOW`], [`SPECULAR`],
//! [`CONTEXT_ZERO`]), the passes run, in this order:
//!
//! - **Layer 10**, the shadow casters (draw `0x000312ad`), with the
//!   shadow flag: the shadow setup `0x00048ee0`, then the layer-begin
//!   check `0x00044370`, which draws when the casters' list
//!   ([`CASTERS`]) has layer 10 and picks the shadow map. No stencil call
//!   before it.
//! - **Layer 11**, the shadow receivers (`0x00031348`), when the light
//!   casts its shadow, has a specular colour, or its definition has bit 5
//!   ([`DEFINITION_RECEIVERS`]): the view's target, then stencil mode 0.
//!   Without any of the three the block is skipped, its stencil call too.
//! - **The additional casters** `0x00030ed0`, with the shadow flag, the
//!   override's byte `+5` ([`OVERRIDE`]) and a positive
//!   [`ADDITIONAL_COUNT`].
//! - **Layer 14** (`0x000313dd`): target 1, then stencil mode 0.
//! - **Layer 12**, diffuse (`0x0003144a`): the view's target, then
//!   stencil mode 0.
//! - **Layer 13**, specular (`0x000314ca`), with flag bits 0 and 1: the
//!   view's target, then stencil mode 0.
//!
//! Layers 11 to 13 draw when the light's list ([`LIGHT_LIST`]) has the
//! layer in its mask. `0x00015370` mode 0 turns the stencil test off, so
//! no layer reads a stencil an earlier one wrote: what a later layer
//! reads of an earlier one is the shadow map (target 9) and the
//! destination alpha layer 11 writes, both through the GPU's order.

use frame_scene::{LightStep, LightTarget};

use crate::GuestRead;

/// The light's list, whose mask (`+8`) says which of layers 11 to 14 draw.
pub const LIGHT_LIST: u32 = 0x004c_1a60;
/// The shadow casters' list; its mask has layer 10.
pub const CASTERS: u32 = 0x004c_1a78;
/// A list's layer mask.
const MASK: u32 = 8;
/// A pointer; with its byte `+5` set the additional casters run.
pub const OVERRIDE: u32 = 0x0051_0c50;
/// The additional casters' count (`i32`).
pub const ADDITIONAL_COUNT: u32 = 0x004b_a134;

/// `[0x004ba014]`'s bits.
pub const CONTEXT_ZERO: u32 = 1;
pub const SPECULAR: u32 = 2;
pub const SHADOW: u32 = 4;
/// The definition flag that runs layer 11 without a shadow.
pub const DEFINITION_RECEIVERS: u32 = 0x20;

/// What decides the steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// The passes' flags, as the flag code sets them.
    pub flags: u32,
    pub definition_flags: u32,
    /// [`LIGHT_LIST`]'s mask.
    pub receivers: u32,
    /// [`CASTERS`]' mask.
    pub casters: u32,
    /// The additional casters' pair runs.
    pub additional_casters: bool,
}

/// The light's lists and the additional casters' inputs, read at the
/// passes' entry, with the flags its constants computed. `None` when a
/// word does not read. **S**: the passes write neither list mask nor
/// [`ADDITIONAL_COUNT`] before they read them (none of the passes' own
/// writes is to them; the shadow setup's callees are not read).
pub fn read(mem: &dyn GuestRead, flags: u32, definition_flags: u32) -> Option<Inputs> {
    let receivers = mem.read_u32(LIGHT_LIST + MASK)?;
    let casters = mem.read_u32(CASTERS + MASK)?;
    let additional_casters = flags & SHADOW != 0 && {
        let over = mem.read_u32(OVERRIDE)?;
        over != 0
            && (mem.read_u32(over.checked_add(4)?)? >> 8) & 0xff != 0
            && mem.read_u32(ADDITIONAL_COUNT)? as i32 > 0
    };
    Some(Inputs { flags, definition_flags, receivers, casters, additional_casters })
}

/// At most five steps, in the passes' order.
#[derive(Clone, Copy, Debug)]
pub struct Steps {
    steps: [LightStep; 5],
    len: usize,
}

impl Steps {
    pub fn as_slice(&self) -> &[LightStep] {
        &self.steps[..self.len]
    }

    pub fn iter(&self) -> impl Iterator<Item = LightStep> + '_ {
        self.as_slice().iter().copied()
    }

    /// The step of `layer`, when the passes reach it.
    pub fn of(&self, layer: u8) -> Option<LightStep> {
        self.iter().find(|s| s.layer == layer)
    }

    /// The layers that draw, as a mask.
    pub fn drawn(&self) -> u32 {
        self.iter().filter(|s| s.draws).fold(0, |m, s| m | 1 << s.layer)
    }
}

/// The light's steps.
pub fn steps(i: &Inputs) -> Steps {
    let f = i.flags;
    let step = |layer: u8, stencil, target, mask: u32| LightStep { layer, stencil, target, draws: mask & 1 << layer != 0 };
    let receivers = |layer, target| step(layer, Some(0), target, i.receivers);
    let eleven = f & (SHADOW | SPECULAR) != 0 || i.definition_flags & DEFINITION_RECEIVERS != 0;
    let thirteen = f & CONTEXT_ZERO != 0 && f & SPECULAR != 0;
    let reached = [
        (f & SHADOW != 0).then(|| step(10, None, LightTarget::ShadowMap, i.casters)),
        eleven.then(|| receivers(11, LightTarget::View)),
        Some(receivers(14, LightTarget::Target(1))),
        Some(receivers(12, LightTarget::View)),
        thirteen.then(|| receivers(13, LightTarget::View)),
    ];
    let mut out = Steps { steps: [receivers(12, LightTarget::View); 5], len: 0 };
    for s in reached.into_iter().flatten() {
        out.steps[out.len] = s;
        out.len += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    fn layers(i: &Inputs) -> Vec<u8> {
        steps(i).iter().map(|s| s.layer).collect()
    }

    const ALL: u32 = 0x1f << 10;

    #[test]
    fn the_passes_order_is_10_11_14_12_13_and_each_flag_adds_its_layers() {
        let all = Inputs { receivers: ALL, casters: ALL, ..Inputs::default() };
        assert_eq!(layers(&all), [14, 12], "no flags: target 1 and diffuse");
        assert_eq!(layers(&Inputs { flags: SPECULAR, ..all }), [11, 14, 12], "specular without context zero");
        assert_eq!(layers(&Inputs { flags: CONTEXT_ZERO | SPECULAR, ..all }), [11, 14, 12, 13]);
        assert_eq!(layers(&Inputs { flags: CONTEXT_ZERO, ..all }), [14, 12]);
        assert_eq!(layers(&Inputs { definition_flags: DEFINITION_RECEIVERS, ..all }), [11, 14, 12]);
        assert_eq!(layers(&Inputs { flags: SHADOW | CONTEXT_ZERO | SPECULAR, ..all }), [10, 11, 14, 12, 13]);
    }

    #[test]
    fn every_step_after_the_casters_turns_the_stencil_off_first() {
        let s = steps(&Inputs { flags: SHADOW | CONTEXT_ZERO | SPECULAR, receivers: ALL, casters: ALL, ..Inputs::default() });
        let caster = s.of(10).unwrap();
        assert_eq!((caster.stencil, caster.target), (None, LightTarget::ShadowMap));
        for layer in [11, 12, 13, 14] {
            assert_eq!(s.of(layer).unwrap().stencil, Some(0), "layer {layer}");
        }
        assert_eq!(s.of(14).unwrap().target, LightTarget::Target(1));
        for layer in [11, 12, 13] {
            assert_eq!(s.of(layer).unwrap().target, LightTarget::View, "layer {layer}");
        }
    }

    #[test]
    fn a_step_draws_only_with_its_layer_in_its_lists_mask() {
        let i = Inputs { flags: SHADOW | CONTEXT_ZERO | SPECULAR, receivers: 1 << 12 | 1 << 13 | 1 << 10, casters: 1 << 11, ..Inputs::default() };
        let s = steps(&i);
        assert_eq!(layers(&i), [10, 11, 14, 12, 13], "reached either way");
        assert!(!s.of(10).unwrap().draws, "layer 10 reads the casters' mask");
        assert!(!s.of(11).unwrap().draws && !s.of(14).unwrap().draws);
        assert_eq!(s.drawn(), 1 << 12 | 1 << 13, "diffuse and specular, both");
        assert_eq!(steps(&Inputs { casters: 1 << 10, ..i }).drawn(), 1 << 10 | 1 << 12 | 1 << 13);
    }

    #[test]
    fn the_lists_and_the_additional_casters_read_at_the_entry() {
        let mut m = Mem::default();
        m.w(LIGHT_LIST + 8, 3 << 12);
        m.w(CASTERS + 8, 1 << 10);
        let i = read(&m, CONTEXT_ZERO, 0).unwrap();
        assert_eq!((i.receivers, i.casters, i.additional_casters), (3 << 12, 1 << 10, false));
        // Without the shadow flag the override is not read.
        assert!(read(&m, SPECULAR, 0).is_some());
        assert!(read(&m, SHADOW, 0).is_none(), "the override does not read");
        m.w(OVERRIDE, 0x9000);
        m.w(0x9004, 1 << 8);
        m.w(ADDITIONAL_COUNT, 2);
        assert!(read(&m, SHADOW, 0).unwrap().additional_casters);
        m.w(ADDITIONAL_COUNT, u32::MAX);
        assert!(!read(&m, SHADOW, 0).unwrap().additional_casters, "a negative count");
        m.w(ADDITIONAL_COUNT, 2);
        m.w(0x9004, 0xff);
        assert!(!read(&m, SHADOW, 0).unwrap().additional_casters, "byte 5 clear");
        m.w(OVERRIDE, 0);
        assert!(!read(&m, SHADOW, 0).unwrap().additional_casters, "no override");
    }
}
