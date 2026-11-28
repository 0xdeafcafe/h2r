//! Surviving guest state of whole diffuse/specular light groups. The
//! renderer owns the draws and the ordered device operations. This plan
//! identifies the surfaces, viewport and depth range for those operations
//! and mirrors the game's layer, shader-cache and pass words.
//!
//! The typed and L5 routes share the whole-group draw: the sorted builder
//! returns no entries to the guest handlers. The layer wrapper still has
//! effects even with that empty return (`0x00044550`, `0x000445d0`).

use super::{Writes, passes};
use crate::light_passes::{flag, global};
use guest_layout::memory::{GuestMemory, MemFault};

const DIFFUSE: u32 = 1 << 12;
const SPECULAR: u32 = 1 << 13;
const LAYER: u32 = 0x0046_7130;
const LIGHT_LIST: u32 = 0x004c_1a60;
const VIEW_TARGET: u32 = 0x0048_58b8;
const CURRENT_TARGET: u32 = 0x0048_58b4;
const STENCIL_MODE: u32 = 0x0046_7010;
const STENCIL_ENABLE: u32 = 0x004b_843c;
const LIGHT_ACTIVE: u32 = 0x0048_58bc;
const DEPTH_RANGE: u32 = 0x0048_5adc;
const DRAW_LAYER: u32 = 0x0048_58b0;
const DRAW_BEGIN: u32 = 0x0046_70bc;
const DRAW_STATE: u32 = 0x0048_5b48;
const RESET_SCALAR: u32 = 0x0045_dbd4;
const UNIT: u32 = 0x0045_dbc0;
const LAYER_SCALE: u32 = 0x004b_8494;
const VIEW_WINDOW: u32 = 0x0048_5648;
const SURFACE_CACHE: u32 = 0x0051_f3f4;
const VIEW_COLOUR: u32 = 0x0050_935c;
const VIEW_DEPTH: u32 = 0x0050_9364;

/// The view bind is a cache hit in the game. The device owner checks that
/// these exact attachments are bound, executes each SetViewport (including
/// the final one), restores stencil, scissors and clip, and commits the
/// batch before the game-side writes.
#[derive(Clone, Debug)]
pub struct Tail {
    pub surfaces: [u32; 2],
    pub viewport: [u32; 6],
    pub clip: [u32; 2],
    /// The NV method passed to SetRenderState_Simple by mode zero.
    pub clip_header: Option<u32>,
    /// `0x00014bc0` returns the final D3D SetViewport result.
    pub eax: u32,
    writes: Writes,
}

impl Tail {
    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.writes.apply(m)
    }
}

/// The target-0 branch of `0x00014bc0` binds the two view surfaces. With
/// both already cached, `0x0001cd30` leaves even its flag byte alone.
/// For the ordinary unscaled viewport each signed short converts to float
/// and back exactly. Scaled and special-purpose viewports use other arms.
fn view(m: &dyn GuestMemory) -> Option<([u32; 2], [u32; 6])> {
    if m.u32(VIEW_TARGET).ok()? != 0 || m.u16(0x0048_5602).ok()? == 2 {
        return None;
    }
    let surfaces = [m.u32(VIEW_COLOUR).ok()?, m.u32(VIEW_DEPTH).ok()?];
    if surfaces.contains(&0)
        || surfaces != [m.u32(SURFACE_CACHE).ok()?, m.u32(SURFACE_CACHE + 4).ok()?]
    {
        return None;
    }
    let unit = m.u32(UNIT).ok()?;
    if unit != 1.0f32.to_bits() {
        return None;
    }
    if m.u8(0x0048_5607).ok()? != 0
        && m.u8(0x0050_93fc).ok()? == 0
        && (m.u32(0x0046_70c8).ok()? != unit || !(m.f32(0x0045_df78).ok()? <= 1.0))
    {
        return None;
    }
    let coord = |offset| u32::try_from(m.u16(VIEW_WINDOW + offset).ok()? as i16).ok();
    let (top, left) = (coord(0)?, coord(2)?);
    let width = coord(6)?.checked_sub(left)?;
    let height = coord(4)?.checked_sub(top)?;
    if width == 0 || height == 0 {
        return None;
    }
    Some((surfaces, [left, top, width, height, 0, unit]))
}

/// Layer setup resets the common shader context (`0x00016b10`). A cached
/// implementation can run a render-state restore at `0x00018900`. Take
/// only an empty restore list. The guest reads the postprocess state's
/// pointer to the render-state table even for a zero count.
fn layer_writes(m: &dyn GuestMemory, out: &mut Writes) -> Option<()> {
    let implementation = m.u32(DRAW_STATE + 0x24).ok()?;
    if implementation != 0 {
        let range = m.u16(implementation.checked_add(0x11c)?).ok()?;
        if range >> 9 != 0 {
            return None;
        }
        m.u32(m.u32(DRAW_STATE + 0x20).ok()?.checked_add(0x14)?).ok()?;
    }
    // At layer end a different scale can issue a device update. At the
    // unit value the guest takes its clean branch (0x000445d0..0x0004460d).
    if m.u32(LAYER_SCALE).ok()? != m.u32(UNIT).ok()? || !(m.f32(0x0045_dbdc).ok()? > 0.0) {
        return None;
    }
    let scalar = m.u32(RESET_SCALAR).ok()?;
    out.word(LAYER + 4, 1);
    out.word(LAYER + 8, LIGHT_LIST);
    // 0x000156f3: stipple and sample alpha. The device owner applies the
    // Simple and SampleAlpha setters; these are their game-side mirrors.
    out.word(0x004b_8348, 0);
    out.word(0x004b_8474, 0);
    out.word(0x0040_75e4, 0);
    out.byte(DRAW_BEGIN, 1);
    for off in (0xc..=0x24).step_by(4) {
        out.word(DRAW_STATE + off, 0);
    }
    for off in [0, 0x40, 0x1320, 0x1360, 0x13a0, 0x13e0] {
        out.word(DRAW_STATE + off, u32::MAX);
    }
    for off in [0x1330, 0x1370, 0x13b0, 0x13f0] {
        out.word(DRAW_STATE + off, scalar);
    }
    out.word(DRAW_LAYER, u32::MAX);
    out.word(CURRENT_TARGET, u32::MAX);
    Some(())
}

fn mapped(m: &dyn GuestMemory, writes: &Writes) -> Option<()> {
    for w in writes.iter() {
        m.bytes(w.addr, w.bytes.len()).ok()?;
    }
    Some(())
}

/// The common state after one first-person slot or dynamic light. `draws`
/// names the layers actually drawn, after testing the receiver-list mask.
pub fn plan_layers(m: &dyn GuestMemory, draws: u32) -> Option<Tail> {
    if draws & !(DIFFUSE | SPECULAR) != 0 {
        return None;
    }
    let (surfaces, viewport) = view(m)?;
    let clip = [m.u32(DEPTH_RANGE).ok()?, m.u32(DEPTH_RANGE + 4).ok()?];
    let mut writes = Writes::new();
    let clip_header = if draws != 0 {
        layer_writes(m, &mut writes)?;
        Some(m.u32(0x0001_56f6).ok()?)
    } else {
        None
    };
    writes.word(LAYER, 0);
    writes.half(STENCIL_MODE, 0);
    writes.byte(LIGHT_ACTIVE, 0);
    writes.word(STENCIL_ENABLE, 0);
    writes.byte(global::PASSES, 0);
    writes.word(global::PASS_FLAGS, 0);
    mapped(m, &writes)?;
    Some(Tail { surfaces, viewport, clip, clip_header, eax: 0, writes })
}

/// In the dynamic entry the last reached layer leaves its draw bit in
/// the low byte of the shadow argument (0x000313f1 or 0x00031474).
pub fn plan_dynamic(
    m: &dyn GuestMemory,
    passes: &passes::Plan,
    flags: u32,
    draws: u32,
) -> Option<Tail> {
    let both = flag::CONTEXT_ZERO | flag::SPECULAR;
    if flags & !both != 0 || draws & SPECULAR != 0 && flags != both {
        return None;
    }
    let mut tail = plan_layers(m, draws)?;
    let slot = passes.entry_esp.checked_add(0xc)?;
    m.u8(slot).ok()?;
    let last = if flags == both { SPECULAR } else { DIFFUSE };
    tail.writes.byte(slot, u8::from(draws & last != 0));
    Some(tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::FlatMemory;
    use crate::light_constants::Fpu;

    const COLOUR: u32 = 0x0040_1000;
    const DEPTH: u32 = 0x0040_1200;
    const ESP: u32 = 0x0051_1000;
    const SCALAR: u32 = 0x41a3_0000;

    fn fixture() -> FlatMemory {
        let mut m = FlatMemory::new(0x0001_0000, 0x51_0000);
        for (at, value) in [
            (0x0001_56f6, 0x0012_3456),
            (VIEW_COLOUR, COLOUR),
            (VIEW_DEPTH, DEPTH),
            (SURFACE_CACHE, COLOUR),
            (SURFACE_CACHE + 4, DEPTH),
            (UNIT, 1.0f32.to_bits()),
            (LAYER_SCALE, 1.0f32.to_bits()),
            (0x0045_dbdc, 0.0001f32.to_bits()),
            (RESET_SCALAR, SCALAR),
            (DEPTH_RANGE, 0x1111_2222),
            (DEPTH_RANGE + 4, 0x3333_4444),
            (ESP + 0xc, 0xabcd_12fe),
            (STENCIL_MODE, 0x8765_4321),
            (LAYER + 4, 0x1111_1111),
            (LAYER + 8, 0x2222_2222),
            (CURRENT_TARGET, 0x9876_5432),
        ] {
            m.set_u32(at, value).unwrap();
        }
        for (i, v) in [20, 45, 100, 175].into_iter().enumerate() {
            m.set_u16(VIEW_WINDOW + i as u32 * 2, v).unwrap();
        }
        m
    }

    fn passes() -> passes::Plan {
        passes::Plan {
            entry_esp: ESP,
            fpu: Fpu { mxcsr: 0x1f80, x87_control: 0x027f },
            definition: 0,
            context: 0,
            data: 0,
            light: 0,
            position: 0,
        }
    }

    #[test]
    fn a_drawn_layer_resets_the_shader_cache_and_keeps_the_last_draw_byte() {
        let mut m = fixture();
        let implementation = 0x0040_3000;
        let state = 0x0040_4000;
        m.set_u32(DRAW_STATE + 0x24, implementation).unwrap();
        m.set_u32(DRAW_STATE + 0x20, state).unwrap();
        m.set_u16(implementation + 0x11c, 75).unwrap(); // nonzero first, zero count
        m.set_u32(state + 0x14, 0x0040_5000).unwrap();
        let tail = plan_dynamic(&m, &passes(), flag::CONTEXT_ZERO | flag::SPECULAR, DIFFUSE).unwrap();
        assert_eq!(tail.surfaces, [COLOUR, DEPTH]);
        assert_eq!(tail.viewport, [45, 20, 130, 80, 0, 1.0f32.to_bits()]);
        assert_eq!(tail.clip, [0x1111_2222, 0x3333_4444]);
        assert_eq!(tail.eax, 0);
        assert_eq!(tail.clip_header, Some(0x0012_3456));
        tail.write(&mut m).unwrap();
        assert_eq!(m.u32(DRAW_STATE + 0x24).unwrap(), 0);
        assert_eq!(m.u32(DRAW_STATE + 0x20).unwrap(), 0);
        assert_eq!(m.u32(DRAW_STATE + 0x13e0).unwrap(), u32::MAX);
        assert_eq!(m.u32(DRAW_STATE + 0x13f0).unwrap(), SCALAR);
        assert_eq!(m.u32(LAYER + 4).unwrap(), 1);
        assert_eq!(m.u32(LAYER + 8).unwrap(), LIGHT_LIST);
        assert_eq!(m.u32(CURRENT_TARGET).unwrap(), u32::MAX);
        assert_eq!(m.u32(STENCIL_MODE).unwrap(), 0x8765_0000);
        // Layer 13 was reached, but its mask bit was clear.
        assert_eq!(m.u32(ESP + 0xc).unwrap(), 0xabcd_1200);
    }

    #[test]
    fn an_empty_layer_leaves_the_draw_cache_and_current_target_alone() {
        let mut m = fixture();
        m.set_u32(DRAW_STATE + 0x24, u32::MAX).unwrap();
        let tail = plan_dynamic(&m, &passes(), 0, 0).unwrap();
        assert_eq!(tail.clip_header, None);
        tail.write(&mut m).unwrap();
        assert_eq!(m.u32(DRAW_STATE + 0x24).unwrap(), u32::MAX);
        assert_eq!(m.u32(LAYER + 4).unwrap(), 0x1111_1111);
        assert_eq!(m.u32(LAYER + 8).unwrap(), 0x2222_2222);
        assert_eq!(m.u32(CURRENT_TARGET).unwrap(), 0x9876_5432);
        assert_eq!(m.u32(ESP + 0xc).unwrap(), 0xabcd_1200);
    }

    #[test]
    fn an_effective_restore_or_surface_change_refuses_before_writing() {
        let mut m = fixture();
        let implementation = 0x0040_3000;
        m.set_u32(DRAW_STATE + 0x24, implementation).unwrap();
        m.set_u16(implementation + 0x11c, 1 << 9).unwrap();
        let before = m.bytes.clone();
        assert!(plan_layers(&m, DIFFUSE).is_none());
        assert_eq!(m.bytes, before);
        m.set_u32(DRAW_STATE + 0x24, 0).unwrap();
        m.set_u32(SURFACE_CACHE + 4, DEPTH + 8).unwrap();
        assert!(plan_layers(&m, 0).is_none());
        m.set_u32(SURFACE_CACHE + 4, DEPTH).unwrap();
        m.set_u32(LAYER_SCALE, 2.0f32.to_bits()).unwrap();
        assert!(plan_layers(&m, DIFFUSE).is_none());
        assert!(plan_dynamic(&m, &passes(), flag::SHADOW, 0).is_none());
    }
}
