//! Render and texture-stage state, read from the guest's own arrays.
//!
//! **The guest arrays are authoritative.** Halo writes `D3D__RenderState`
//! and `D3D__TextureState` directly from game code (386 and 432 `.text`
//! accesses; see `docs/graphics/d3d.md`), then either calls a D3D helper that pushes
//! the matching NV2A method or only sets a bit in `D3D__DirtyFlags` for the
//! lazy commit. Rather than intercept every one of those sites, the device
//! snapshots both arrays at each draw and translates the snapshot. Overridden
//! setters (`SetRenderState_*`, `SetTextureState_*`) therefore only have to
//! store into the arrays, exactly as the originals do.

use gfx::state::{RenderState, RenderStateType, TextureStageState, TextureStageStateType};
use gfx::xdk::RenderStateTable;

use crate::guest::{GuestError, GuestMemory};
use crate::layout::{GuestLayout, RENDER_STATE_SLOTS, STAGE_SLOTS, STAGES};

/// Reads `out.len()` words at `addr` into `out`, same limits as
/// [`GuestMemory::read_words`]: one access over the whole range, so a fault
/// reports all of it. Straight into `out`, without a staging buffer (this
/// runs for every draw); `out` is unspecified on an error.
fn read_words_into(mem: &dyn GuestMemory, addr: u32, out: &mut [u32]) -> Result<(), GuestError> {
    let bytes: &mut [u8] = bytemuck::cast_slice_mut(out);
    crate::guest::check_range(addr, bytes.len())?;
    mem.read(addr, bytes)?;
    // The guest's words are little endian; a no-op on a little-endian host.
    for w in out.iter_mut() {
        *w = u32::from_le(*w);
    }
    Ok(())
}

/// Raw words of both state arrays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateWords {
    /// `D3D__RenderState`, guest numbering.
    pub render: [u32; RENDER_STATE_SLOTS],
    /// `D3D__TextureState[stage][state]`.
    pub stages: [[u32; STAGE_SLOTS]; STAGES],
}

/// A state value the translation rejected (reported, the previous/default
/// value is kept for that field).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadState {
    /// Render state, guest slot and value.
    Render { slot: u32, value: u32 },
    /// Texture-stage state.
    Stage { stage: u32, state: u32, value: u32 },
}

/// The translated state for one draw.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawState {
    /// Pipeline-relevant render state.
    pub render: RenderState,
    /// Per-stage sampler and fixed-function state.
    pub stages: [TextureStageState; STAGES],
    /// Rejected values.
    pub bad: Vec<BadState>,
}

/// Guest-numbered render-state slots the device itself cares about.
pub mod slot {
    /// `D3DRS_PSTEXTUREMODES` (complex group).
    pub const PS_TEXTURE_MODES: u32 = 136;
    /// `D3DRS_ZENABLE`.
    pub const ZENABLE: u32 = 143;
    /// `D3DRS_STENCILENABLE`.
    pub const STENCILENABLE: u32 = 144;
    /// `D3DRS_CULLMODE`.
    pub const CULLMODE: u32 = 147;
    /// `D3DRS_POLYGONOFFSETZSLOPESCALE` (float).
    pub const POLYGON_OFFSET_ZSLOPESCALE: u32 = 77;
    /// `D3DRS_POLYGONOFFSETZOFFSET` (float).
    pub const POLYGON_OFFSET_ZOFFSET: u32 = 78;
    /// `D3DRS_POINTOFFSETENABLE`.
    pub const POINT_OFFSET_ENABLE: u32 = 79;
    /// `D3DRS_WIREFRAMEOFFSETENABLE`.
    pub const WIREFRAME_OFFSET_ENABLE: u32 = 80;
    /// `D3DRS_SOLIDOFFSETENABLE`.
    pub const SOLID_OFFSET_ENABLE: u32 = 81;
    /// `D3DRS_ZBIAS`.
    pub const ZBIAS: u32 = 149;
    /// `D3DRS_SHADOWFUNC` (a `D3DCMP_*` value; `SetRenderState_ShadowFunc`
    /// `0x3f5330`): the shadow-map comparison for depth-format textures.
    pub const SHADOWFUNC: u32 = 156;
    /// Slots 0..57 mirror the first 57 dwords of a `D3DPIXELSHADERDEF`
    /// (`PSALPHAINPUTS0` .. `PSINPUTTEXTURE`).
    pub const PS_DEF_SLOTS: u32 = 57;
}

/// `D3DTSS_*` indices (`X_D3DTSS_*` numbering, Cxbx `XbD3D8Types.h`) that
/// [`TextureStageState`] does not model but shader translation needs.
pub mod stage_slot {
    /// `D3DTSS_COLORKEYOP`.
    pub const COLORKEYOP: usize = 9;
    /// `D3DTSS_COLORSIGN`.
    pub const COLORSIGN: usize = 10;
    /// `D3DTSS_ALPHAKILL`.
    pub const ALPHAKILL: usize = 11;
    /// `D3DTSS_TEXTURETRANSFORMFLAGS`.
    pub const TEXTURETRANSFORMFLAGS: usize = 21;
    /// `D3DTSS_BUMPENVMAT00` (then 01, 11, 10).
    pub const BUMPENVMAT00: usize = 22;
    /// `D3DTSS_BUMPENVLSCALE`.
    pub const BUMPENVLSCALE: usize = 26;
    /// `D3DTSS_BUMPENVLOFFSET`.
    pub const BUMPENVLOFFSET: usize = 27;
    /// `D3DTSS_COLORKEYCOLOR`.
    pub const COLORKEYCOLOR: usize = 30;
}

/// Per-stage state outside [`TextureStageState`], decoded from the raw words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageExtras {
    /// `D3DTSS_COLORKEYOP`.
    pub color_key_op: u32,
    /// `D3DTSS_COLORSIGN` bits.
    pub color_sign: u32,
    /// `D3DTSS_ALPHAKILL` (`D3DTALPHAKILL_ENABLE` = 4).
    pub alpha_kill: u32,
    /// `D3DTSS_TEXTURETRANSFORMFLAGS`.
    pub texture_transform_flags: u32,
    /// Bump matrix `[m00, m01, m10, m11]`.
    pub bump_env_mat: [f32; 4],
    /// Luminance scale.
    pub bump_env_lscale: f32,
    /// Luminance offset.
    pub bump_env_loffset: f32,
    /// `D3DTSS_COLORKEYCOLOR`.
    pub color_key_color: u32,
}

impl StateWords {
    /// Stage state that only the raw words carry.
    pub fn stage_extras(&self) -> [StageExtras; STAGES] {
        use stage_slot::*;
        std::array::from_fn(|n| {
            let w = &self.stages[n];
            let f = |i: usize| f32::from_bits(w[i]);
            StageExtras {
                color_key_op: w[COLORKEYOP],
                color_sign: w[COLORSIGN],
                alpha_kill: w[ALPHAKILL],
                texture_transform_flags: w[TEXTURETRANSFORMFLAGS],
                bump_env_mat: [
                    f(BUMPENVMAT00),
                    f(BUMPENVMAT00 + 1),
                    f(BUMPENVMAT00 + 3),
                    f(BUMPENVMAT00 + 2),
                ],
                bump_env_lscale: f(BUMPENVLSCALE),
                bump_env_loffset: f(BUMPENVLOFFSET),
                color_key_color: w[COLORKEYCOLOR],
            }
        })
    }

    /// Reads both arrays.
    pub fn read(mem: &dyn GuestMemory, layout: &GuestLayout) -> Result<Self, GuestError> {
        // Straight into arrays: this runs for every draw.
        let mut render = [0; RENDER_STATE_SLOTS];
        read_words_into(mem, layout.render_state, &mut render)?;
        // One 512-byte access, as `read_words` made: the whole range is
        // checked first and a fault reports the whole access.
        let mut stages = [[0; STAGE_SLOTS]; STAGES];
        read_words_into(mem, layout.texture_state, stages.as_flattened_mut())?;
        Ok(Self { render, stages })
    }

    /// Writes both arrays.
    pub fn write(&self, mem: &mut dyn GuestMemory, layout: &GuestLayout) -> Result<(), GuestError> {
        for (i, w) in self.render.iter().enumerate() {
            mem.write_u32(layout.render_state + 4 * i as u32, *w)?;
        }
        for (s, stage) in self.stages.iter().enumerate() {
            for (i, w) in stage.iter().enumerate() {
                mem.write_u32(layout.texture_state_slot(s as u32, i as u32), *w)?;
            }
        }
        Ok(())
    }

    /// The D3D defaults, encoded as the guest stores them, for a device with
    /// (`auto_depth`) or without an automatic depth buffer.
    pub fn defaults(auto_depth: bool) -> Self {
        let table = RenderStateTable::default();
        let mut render = [0u32; RENDER_STATE_SLOTS];
        let mut set = |canonical: u32, v: u32| {
            if let Some(g) = table.canonical_to_guest(canonical) {
                render[g as usize] = v;
            }
        };
        let one = 1.0f32.to_bits();
        for (canonical, v) in [
            (57, 0x203),              // ZFUNC = LESSEQUAL
            (58, 0x207),              // ALPHAFUNC = ALWAYS
            (62, 1),                  // SRCBLEND = ONE
            (63, 0),                  // DESTBLEND = ZERO
            (64, 1),                  // ZWRITEENABLE
            (66, 0x1D01),             // SHADEMODE = GOURAUD
            (67, 0x0101_0101),        // COLORWRITEENABLE = ALL
            (68, 0x1E00),             // STENCILZFAIL = KEEP
            (69, 0x1E00),             // STENCILPASS = KEEP
            (70, 0x207),              // STENCILFUNC = ALWAYS
            (72, 0xFF),               // STENCILMASK
            (73, 0xFF),               // STENCILWRITEMASK
            (74, 0x8006),             // BLENDOP = ADD
            (95, one),                // FOGEND
            (96, one),                // FOGDENSITY
            (102, 1),                 // LIGHTING
            (116, one),               // POINTSIZE
            (117, 0),                 // POINTSIZE_MIN
            (120, one),               // POINTSCALE_A
            (123, 64.0f32.to_bits()), // POINTSIZE_MAX
            (139, 0x1B02),            // FILLMODE = SOLID
            (140, 0x1B02),            // BACKFILLMODE = SOLID
            (143, auto_depth as u32), // ZENABLE
            (145, 0x1E00),            // STENCILFAIL = KEEP
            (146, 0x900),             // FRONTFACE = CW
            (147, 0x901),             // CULLMODE = CCW
            (148, 0xFFFF_FFFF),       // TEXTUREFACTOR
            (152, 1),                 // MULTISAMPLEANTIALIAS
            (153, 0xFFFF_FFFF),       // MULTISAMPLEMASK
            (158, one),               // LINEWIDTH
        ] {
            set(canonical, v);
        }
        let mut stages = [[0u32; STAGE_SLOTS]; STAGES];
        for (n, s) in stages.iter_mut().enumerate() {
            s[0] = 1; // ADDRESSU = WRAP
            s[1] = 1;
            s[2] = 1;
            s[3] = 1; // MAGFILTER = POINT
            s[4] = 1;
            s[5] = 0; // MIPFILTER = NONE
            s[8] = 1; // MAXANISOTROPY
            s[12] = if n == 0 { 4 } else { 1 }; // COLOROP = MODULATE / DISABLE
            s[13] = 1; // COLORARG0 = CURRENT
            s[14] = 2; // COLORARG1 = TEXTURE
            s[15] = 1; // COLORARG2 = CURRENT
            s[16] = if n == 0 { 2 } else { 1 }; // ALPHAOP = SELECTARG1 / DISABLE
            s[17] = 1;
            s[18] = 2;
            s[19] = 1;
            s[20] = 1; // RESULTARG = CURRENT
            s[28] = n as u32; // TEXCOORDINDEX
        }
        Self { render, stages }
    }

    /// Translates to the gfx state model. Invalid values are collected, not
    /// fatal: the field keeps its default and the caller reports them.
    pub fn translate(&self, table: &RenderStateTable) -> DrawState {
        Translator::new(table).translate(self)
    }
}

/// [`StateWords::translate`] with the slot-to-type lookups done once: the
/// device translates every draw, and the table never changes under it.
#[derive(Clone, Debug)]
pub struct Translator {
    /// Typed render state per guest slot (`None`: no such slot, or a slot
    /// the model does not track).
    render: Vec<Option<RenderStateType>>,
    /// Typed stage state per slot (`None`: not modelled).
    stage: [Option<TextureStageStateType>; STAGE_SLOTS],
}

impl Translator {
    /// Resolves the slot types for the build described by `table`.
    pub fn new(table: &RenderStateTable) -> Self {
        Self {
            render: (0..RENDER_STATE_SLOTS as u32)
                .map(|g| {
                    RenderStateType::from_guest_index(table, g)
                        .filter(|t| !matches!(t, RenderStateType::Other(_)))
                })
                .collect(),
            stage: std::array::from_fn(|i| {
                let ty = TextureStageStateType::from_index(i as u32);
                (!matches!(ty, TextureStageStateType::Other(_))).then_some(ty)
            }),
        }
    }

    /// Translates `words`; identical to [`StateWords::translate`].
    pub fn translate(&self, words: &StateWords) -> DrawState {
        let mut bad = Vec::new();
        let render = self.translate_render(&words.render, &mut bad);
        let stages = std::array::from_fn(|s| self.translate_stage(s, &words.stages[s], &mut bad));
        DrawState {
            render,
            stages,
            bad,
        }
    }

    /// The render-state half of [`Self::translate`]: a function of the
    /// render words alone.
    fn translate_render(
        &self,
        words: &[u32; RENDER_STATE_SLOTS],
        bad: &mut Vec<BadState>,
    ) -> RenderState {
        let mut render = RenderState::default();
        for (g, (&v, ty)) in words.iter().zip(&self.render).enumerate() {
            let Some(ty) = *ty else { continue };
            if render.set_raw(ty, v).is_err() {
                bad.push(BadState::Render {
                    slot: g as u32,
                    value: v,
                });
            }
        }
        render
    }

    /// Stage `s` of [`Self::translate`]: a function of that stage's words
    /// alone.
    fn translate_stage(
        &self,
        s: usize,
        words: &[u32; STAGE_SLOTS],
        bad: &mut Vec<BadState>,
    ) -> TextureStageState {
        let mut st = TextureStageState::default_for_stage(s as u32);
        for (i, (&v, ty)) in words.iter().zip(&self.stage).enumerate() {
            let Some(ty) = *ty else { continue };
            if st.set_raw(ty, v).is_err() {
                bad.push(BadState::Stage {
                    stage: s as u32,
                    state: i as u32,
                    value: v,
                });
            }
        }
        st
    }
}

/// One translated part (the render words, or one stage's words): its words,
/// what they translate to, and the values rejected.
#[derive(Clone, Debug)]
struct Part<W, T> {
    words: W,
    state: T,
    bad: Vec<BadState>,
}

/// [`Translator::translate`] for a run of draws, re-translating only the
/// parts whose words changed since the last draw.
///
/// The render state is a function of the render words and each stage's
/// state of that stage's words ([`Translator::translate_render`],
/// [`Translator::translate_stage`]); rejected values are listed render
/// first, then stage by stage, as `translate` lists them. Halo changes a
/// few words between most draws, so most parts are reused.
#[derive(Clone, Debug, Default)]
pub struct TranslateMemo {
    render: Option<Part<[u32; RENDER_STATE_SLOTS], RenderState>>,
    stages: [Option<Part<[u32; STAGE_SLOTS], TextureStageState>>; STAGES],
}

impl TranslateMemo {
    /// Equal to `t.translate(words)`.
    pub fn translate(&mut self, t: &Translator, words: &StateWords) -> DrawState {
        if self.render.as_ref().is_none_or(|p| p.words != words.render) {
            let mut bad = Vec::new();
            let state = t.translate_render(&words.render, &mut bad);
            self.render = Some(Part {
                words: words.render,
                state,
                bad,
            });
        }
        for (s, part) in self.stages.iter_mut().enumerate() {
            if part.as_ref().is_none_or(|p| p.words != words.stages[s]) {
                let mut bad = Vec::new();
                let state = t.translate_stage(s, &words.stages[s], &mut bad);
                *part = Some(Part {
                    words: words.stages[s],
                    state,
                    bad,
                });
            }
        }
        let render = self.render.as_ref().expect("translated above");
        let stages = self
            .stages
            .each_ref()
            .map(|p| p.as_ref().expect("translated above"));
        let mut bad = render.bad.clone();
        for p in &stages {
            bad.extend_from_slice(&p.bad);
        }
        DrawState {
            render: render.state,
            stages: stages.map(|p| p.state),
            bad,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::state::{BlendFactor, CullMode, DepthBufferMode};

    #[test]
    fn defaults_translate_cleanly_to_gfx_defaults() {
        let t = RenderStateTable::default();
        let d = StateWords::defaults(true).translate(&t);
        assert!(d.bad.is_empty(), "{:?}", d.bad);
        assert_eq!(d.render, RenderState::default());
        for (n, s) in d.stages.iter().enumerate() {
            assert_eq!(*s, TextureStageState::default_for_stage(n as u32));
        }
        let d = StateWords::defaults(false).translate(&t);
        assert_eq!(d.render.depth_mode, DepthBufferMode::Disabled);
    }

    /// D3DRS_DEPTHCLIPCONTROL reaches the draw's state, so a clamped draw
    /// gets an unclipped-depth pipeline.
    #[test]
    fn depth_clip_control_translates_to_depth_clamp() {
        let t = RenderStateTable::default();
        let slot = t.canonical_to_guest(82).expect("DEPTHCLIPCONTROL slot") as usize;
        let mut w = StateWords::defaults(true);
        assert!(!w.translate(&t).render.depth_clamp);
        w.render[slot] = 0x10;
        let d = w.translate(&t);
        assert!(d.bad.is_empty(), "{:?}", d.bad);
        assert!(d.render.depth_clamp);
    }

    /// The memo answers what a fresh translation answers, rejected values
    /// and their order included, over a run of draws that each change a
    /// few render or stage words (to valid, invalid and repeated values),
    /// and when nothing changes.
    #[test]
    fn memo_matches_a_fresh_translation_draw_after_draw() {
        let table = RenderStateTable::default();
        let t = Translator::new(&table);
        let mut memo = TranslateMemo::default();
        let mut w = StateWords::defaults(true);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for draw in 0..4000 {
            for _ in 0..(next() % 4) {
                let r = next();
                // Small values are mostly valid enums and flags; others
                // are mostly rejected.
                let v = if r & 1 == 0 {
                    (r >> 8) as u32 % 9
                } else {
                    (r >> 8) as u32
                };
                if (r >> 1) % 3 == 0 {
                    let stage = (r >> 40) as usize % STAGES;
                    w.stages[stage][(r >> 48) as usize % STAGE_SLOTS] = v;
                } else {
                    w.render[(r >> 40) as usize % RENDER_STATE_SLOTS] = v;
                }
            }
            if draw % 500 == 499 {
                w = StateWords::defaults(draw % 1000 == 499);
            }
            assert_eq!(memo.translate(&t, &w), t.translate(&w), "draw {draw}");
        }
    }

    #[test]
    fn translator_is_the_free_function_and_reads_match_writes() {
        let t = RenderStateTable::default();
        let mut w = StateWords::defaults(true);
        w.render[57] = 0x1234; // bogus ZFUNC
        w.stages[2][12] = 0x7777; // bogus COLOROP
        let tr = Translator::new(&t);
        assert_eq!(tr.translate(&w), w.translate(&t));
        assert_eq!(tr.translate(&w).bad.len(), 2);
        let mut m = crate::guest::SparseMemory::new();
        let l = GuestLayout::HALO2;
        m.map(l.texture_state & !0xFFF, 0x5000);
        w.write(&mut m, &l).unwrap();
        assert_eq!(StateWords::read(&m, &l).unwrap(), w);
    }

    /// Every word distinct, every byte of it significant: the read puts
    /// each guest word, little endian, in its own slot and stage.
    #[test]
    fn state_read_places_every_word() {
        let l = GuestLayout::HALO2;
        let mut m = crate::guest::SparseMemory::new();
        m.map(l.texture_state & !0xFFF, 0x5000);
        let word = |i: u32| 0x0102_0304u32.wrapping_mul(i + 1) ^ (i << 24);
        for i in 0..RENDER_STATE_SLOTS as u32 {
            m.write_u32(l.render_state + 4 * i, word(i)).unwrap();
        }
        for s in 0..STAGES as u32 {
            for i in 0..STAGE_SLOTS as u32 {
                m.write_u32(l.texture_state_slot(s, i), word(1000 + 32 * s + i))
                    .unwrap();
            }
        }
        let w = StateWords::read(&m, &l).unwrap();
        for i in 0..RENDER_STATE_SLOTS {
            assert_eq!(w.render[i], word(i as u32), "render {i}");
        }
        for s in 0..STAGES {
            for i in 0..STAGE_SLOTS {
                assert_eq!(
                    w.stages[s][i],
                    word(1000 + 32 * s as u32 + i as u32),
                    "stage {s} slot {i}"
                );
            }
        }
    }

    #[test]
    fn state_read_checks_the_whole_texture_array_with_a_custom_layout() {
        // A layout whose texture array runs off the top of the address
        // space: the access is one 512-byte read, rejected as a whole
        // (no panic, no wrap to address zero).
        let l = GuestLayout {
            texture_state: 0xFFFF_FF80,
            ..GuestLayout::HALO2
        };
        let mut m = crate::guest::SparseMemory::new();
        m.map(l.render_state, RENDER_STATE_SLOTS as u32 * 4);
        m.map(0xFFFF_F000, 0x1000);
        m.map(0, 0x1000);
        assert_eq!(
            StateWords::read(&m, &l).unwrap_err(),
            GuestError {
                addr: 0xFFFF_FF80,
                len: 512
            }
        );
    }

    #[test]
    fn guest_writes_show_up_and_bad_values_are_reported() {
        let t = RenderStateTable::default();
        let mut w = StateWords::defaults(true);
        w.render[59] = 1; // ALPHABLENDENABLE
        w.render[62] = 0x302; // SRCBLEND = SRCALPHA
        w.render[slot::CULLMODE as usize] = 0; // NONE
        w.render[57] = 0x1234; // bogus ZFUNC
        let d = w.translate(&t);
        assert!(d.render.blend_enable);
        assert_eq!(d.render.src_blend, BlendFactor::SrcAlpha);
        assert_eq!(d.render.cull_mode, CullMode::None);
        assert_eq!(
            d.bad,
            vec![BadState::Render {
                slot: 57,
                value: 0x1234
            }]
        );
    }
}
