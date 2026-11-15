//! NV2A register state that D3D's internal fragments push and the device
//! keeps: viewport scale and offset, depth clip range, `SET_CONTROL0`,
//! transform-constant and transform-program loads, the polygon stipple
//! pattern at `0x1480`, and the two clear-value registers D3D also uses as
//! software-method operands.
//!
//! These methods reach the push buffer from D3D code the game calls directly
//! (the viewport/transform fragments `0x3f6190`, `0x3f6390`, `0x3f6f40` and
//! `SetStipple` `0x3f8cc0`), from game code (the stipple pattern inline at
//! `0x1e4ae`, an inlined `InsertCallback` at `0x2460e`, the vertex-cache
//! break at `0x43a1a`) and from the device's own copies of them (see
//! `device/fragments.rs`). The decoder applies each one in program order
//! when the push buffer drains, like the inline state methods.
//!
//! Constant and program loads follow the hardware cursor model (as xemu
//! models it): `SET_TRANSFORM_CONSTANT_LOAD` (`0x1EA4`) sets the register
//! cursor, every word written to `SET_TRANSFORM_CONSTANT` (`0x0B80..0x0BFC`)
//! lands in component `slot % 4` of the cursor register, and the cursor
//! advances after component 3. Programs work the same way with
//! `SET_TRANSFORM_PROGRAM_LOAD` (`0x1E9C`) and `SET_TRANSFORM_PROGRAM`
//! (`0x0B00..0x0B7C`). `SET_VIEWPORT_OFFSET`/`SCALE` also land in constant
//! registers 59 and 58 (hardware numbering, `c-37`/`c-38`), where vertex
//! programs read them.

use crate::shader::{CONSTANTS, PROGRAM_SLOTS};

/// NV2A method offsets this module understands.
pub mod method {
    /// `NV097_SET_CONTROL0`.
    pub const CONTROL0: u32 = 0x0290;
    /// `NV097_SET_CLIP_MIN`.
    pub const CLIP_MIN: u32 = 0x0394;
    /// `NV097_SET_CLIP_MAX`.
    pub const CLIP_MAX: u32 = 0x0398;
    /// `NV097_SET_VIEWPORT_OFFSET` (4 words).
    pub const VIEWPORT_OFFSET: u32 = 0x0A20;
    /// `NV097_SET_VIEWPORT_SCALE` (4 words).
    pub const VIEWPORT_SCALE: u32 = 0x0AF0;
    /// `NV097_SET_TRANSFORM_PROGRAM` (32 words).
    pub const TRANSFORM_PROGRAM: u32 = 0x0B00;
    /// `NV097_SET_TRANSFORM_CONSTANT` (32 words).
    pub const TRANSFORM_CONSTANT: u32 = 0x0B80;
    /// `NV097_SET_STIPPLE_CONTROL` (polygon stipple on/off). D3D pushes it
    /// for `D3DRS_STIPPLEENABLE` through `D3DSIMPLERENDERSTATEENCODE`, so
    /// it normally drains as a render-state mirror; this is the fallback.
    pub const STIPPLE_CONTROL: u32 = 0x147C;
    /// `NV097_SET_STIPPLE_PATTERN` (32 words, one 32-pixel row each):
    /// `SetStipple` `0x3f8cc0` and inline at `0x1e4ae`.
    pub const STIPPLE_PATTERN: u32 = 0x1480;
    /// `NV097_BREAK_VERTEX_BUFFER_CACHE`. Game code pushes it (value 0, at
    /// `0x43a1a`) after writing vertices the GPU may have cached. The
    /// device reads vertex data from guest memory at every draw, so it has
    /// no cache to break.
    pub const BREAK_VERTEX_BUFFER_CACHE: u32 = 0x1710;
    /// `NV097_SET_VERTEX_DATA_ARRAY_OFFSET` (16 words, one per attribute
    /// register): where the GPU fetches the register's array from (a
    /// physical address, bit 31 picking the DMA context). `CDevice_SetStateVB`
    /// (`0x401030`) pushes one per enabled attribute: the vertex buffer's
    /// `Data` plus the declaration and stream offsets (plus `base * stride`
    /// for indexed draws).
    pub const VERTEX_DATA_ARRAY_OFFSET: u32 = 0x1720;
    /// `NV097_SET_VERTEX_DATA_ARRAY_FORMAT` (16 words): bits 3:0 the type
    /// (the `D3DVSDT_*` low nibble), 7:4 the component count (0 disables the
    /// register), 31:8 the stride. `CDevice_SetStateVB` pushes all 16 when
    /// the vertex-format dirty bit (`0x10`) is set. Inline arrays
    /// (`0x1818`) are unpacked with these.
    pub const VERTEX_DATA_ARRAY_FORMAT: u32 = 0x1760;
    /// `NV097_SET_EYE_VECTOR` (3 floats, into `NV_PGRAPH_EYEVEC0..2`): the
    /// constant eye vector of the texture shader's `DOT_RFLCT_SPEC_CONST`
    /// mode. No retail Halo 2 shader uses that mode, and no push of this
    /// method has been seen; it is kept so a push lands in
    /// [`HwState::eye_vector`] instead of stopping as unknown.
    pub const EYE_VECTOR: u32 = 0x181C;
    /// `NV097_CLEAR_REPORT_VALUE`: zeroes the report counter its parameter
    /// names (1, `ZPASS_PIXEL_CNT`, the only type D3D uses). Halo's inlined
    /// `BeginVisibilityTest` (`0x20cec`) pushes it with
    /// [`ZPASS_PIXEL_COUNT_ENABLE`] = 1 in one two-word command. The device
    /// handles it in the drain (it restarts the backend's sample count);
    /// it is not register state.
    pub const CLEAR_REPORT_VALUE: u32 = 0x17C8;
    /// `NV097_SET_ZPASS_PIXEL_COUNT_ENABLE`: while set, draws add the
    /// samples that pass the depth and stencil tests to the zpass counter.
    pub const ZPASS_PIXEL_COUNT_ENABLE: u32 = 0x17CC;
    /// `NV097_GET_REPORT` (bits 31:24 the type, 23:0 an offset into the
    /// report DMA context): writes the counter to memory. Only
    /// `EndVisibilityTest` (`0x3f85d0`) pushes it, and the device overrides
    /// that, so the drain does not decode it (an unknown method stops).
    pub const GET_REPORT: u32 = 0x17D0;
    /// `NV097_SET_ZSTENCIL_CLEAR_VALUE`. D3D (and the inlined
    /// `InsertCallback` at `0x2460e`) also loads it with a software-method
    /// operand: the callback function, or a register offset.
    pub const ZSTENCIL_CLEAR_VALUE: u32 = 0x1D8C;
    /// `NV097_SET_COLOR_CLEAR_VALUE`. The second software-method operand
    /// (the callback context, or a register value). It is also the
    /// placeholder method of the unused `D3DSIMPLERENDERSTATEENCODE` slots.
    pub const COLOR_CLEAR_VALUE: u32 = 0x1D90;
    /// `NV097_SET_TRANSFORM_EXECUTION_MODE`.
    pub const EXECUTION_MODE: u32 = 0x1E94;
    /// `NV097_SET_TRANSFORM_PROGRAM_CXT_WRITE_EN`.
    pub const CXT_WRITE_EN: u32 = 0x1E98;
    /// `NV097_SET_TRANSFORM_PROGRAM_LOAD`.
    pub const PROGRAM_LOAD: u32 = 0x1E9C;
    /// `NV097_SET_TRANSFORM_PROGRAM_START`.
    pub const PROGRAM_START: u32 = 0x1EA0;
    /// `NV097_SET_TRANSFORM_CONSTANT_LOAD`.
    pub const CONSTANT_LOAD: u32 = 0x1EA4;
}

/// Constant register (hardware numbering, 0..192) that `SET_VIEWPORT_SCALE`
/// writes (`c-38`).
pub const VIEWPORT_SCALE_REGISTER: usize = 58;
/// Constant register that `SET_VIEWPORT_OFFSET` writes (`c-37`).
pub const VIEWPORT_OFFSET_REGISTER: usize = 59;

/// The register state, as last pushed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HwState {
    /// `SET_CONTROL0` (`0x100001` base; `0x10000` w-buffer, `0x1000` float
    /// depth, `0x10000000` YUV).
    pub control0: Option<u32>,
    /// `SET_VIEWPORT_OFFSET`.
    pub viewport_offset: [f32; 4],
    /// `SET_VIEWPORT_SCALE`.
    pub viewport_scale: [f32; 4],
    /// `SET_CLIP_MIN` (depth units).
    pub clip_min: f32,
    /// `SET_CLIP_MAX` (depth units).
    pub clip_max: f32,
    /// Transform-constant cursor (register).
    pub constant_load: u32,
    /// Transform-program cursor (instruction slot).
    pub program_load: u32,
    /// `SET_TRANSFORM_PROGRAM_START`, if pushed.
    pub program_start: Option<u32>,
    /// `SET_TRANSFORM_EXECUTION_MODE`.
    pub execution_mode: u32,
    /// `SET_TRANSFORM_PROGRAM_CXT_WRITE_EN`.
    pub cxt_write_enable: u32,
    /// `SET_STIPPLE_CONTROL`, if it arrived as a plain method (see
    /// [`method::STIPPLE_CONTROL`]).
    pub stipple_control: Option<u32>,
    /// `SET_STIPPLE_PATTERN`: 32 rows of 32 pixels, bit `x` of row `y` set
    /// where pixel `(x mod 32, y mod 32)` is drawn while stippling is
    /// enabled. Halo picks one of 17 dither levels (the table at `0x487288`,
    /// selected by `0x1c290` from an opacity) for screen-door fades.
    pub stipple_pattern: [u32; 32],
    /// `SET_ZSTENCIL_CLEAR_VALUE` as last pushed.
    pub zstencil_clear_value: u32,
    /// `SET_COLOR_CLEAR_VALUE` as last pushed.
    pub color_clear_value: u32,
    /// `SET_VERTEX_DATA_ARRAY_FORMAT`, per attribute register.
    pub vertex_format: [u32; 16],
    /// `SET_VERTEX_DATA_ARRAY_OFFSET`, per attribute register. Recorded
    /// only: nothing draws from GPU-side arrays (`DRAW_ARRAYS` and the
    /// `ARRAY_ELEMENT` methods stop as unknown).
    pub vertex_offset: [u32; 16],
    /// `SET_ZPASS_PIXEL_COUNT_ENABLE`: draws issued while it is set count
    /// the samples they pass ([`crate::backend::Backend::report_pixel_count`]).
    pub zpass_count_enable: bool,
    /// `SET_EYE_VECTOR`: the eye vector `DOT_RFLCT_SPEC_CONST` reflects
    /// (the pixel uniforms' `eye`).
    pub eye_vector: [f32; 3],
}

/// The reset value of every `SET_VERTEX_DATA_ARRAY_FORMAT` register: float,
/// no components (`D3DVSDT_NONE`), stride 0.
pub const VERTEX_FORMAT_DISABLED: u32 = 0x02;

impl Default for HwState {
    fn default() -> Self {
        Self {
            control0: None,
            viewport_offset: [0.0; 4],
            viewport_scale: [0.0; 4],
            clip_min: 0.0,
            clip_max: 0.0,
            constant_load: 0,
            program_load: 0,
            program_start: None,
            execution_mode: 0,
            cxt_write_enable: 0,
            stipple_control: None,
            stipple_pattern: [0; 32],
            zstencil_clear_value: 0,
            color_clear_value: 0,
            vertex_format: [VERTEX_FORMAT_DISABLED; 16],
            vertex_offset: [0; 16],
            zpass_count_enable: false,
            eye_vector: [0.0; 3],
        }
    }
}

/// A load ran past the end of constant or program memory.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{what} load past the end: slot {slot}")]
pub struct HwOverflow {
    /// `"transform constant"` or `"transform program"`.
    pub what: &'static str,
    /// The cursor.
    pub slot: u32,
}

/// Where constant and program writes go.
pub struct Memories<'a> {
    /// 192 constant registers as dwords.
    pub constants: &'a mut [u32; CONSTANTS * 4],
    /// 136 program slots as dwords.
    pub program: &'a mut [u32; PROGRAM_SLOTS * 4],
}

impl HwState {
    /// Applies one method write. `Ok(false)` means the method is not one of
    /// these (the caller decides what that means).
    pub fn apply(&mut self, m: u32, w: u32, mem: &mut Memories<'_>) -> Result<bool, HwOverflow> {
        use method::*;
        let f = f32::from_bits(w);
        match m {
            CONTROL0 => self.control0 = Some(w),
            CLIP_MIN => self.clip_min = f,
            CLIP_MAX => self.clip_max = f,
            _ if (VIEWPORT_OFFSET..VIEWPORT_OFFSET + 16).contains(&m) => {
                let i = ((m - VIEWPORT_OFFSET) / 4) as usize;
                self.viewport_offset[i] = f;
                mem.constants[VIEWPORT_OFFSET_REGISTER * 4 + i] = w;
            }
            _ if (VIEWPORT_SCALE..VIEWPORT_SCALE + 16).contains(&m) => {
                let i = ((m - VIEWPORT_SCALE) / 4) as usize;
                self.viewport_scale[i] = f;
                mem.constants[VIEWPORT_SCALE_REGISTER * 4 + i] = w;
            }
            _ if (TRANSFORM_PROGRAM..TRANSFORM_PROGRAM + 0x80).contains(&m) => {
                let c = ((m - TRANSFORM_PROGRAM) / 4) % 4;
                let slot = self.program_load;
                if slot as usize >= PROGRAM_SLOTS {
                    return Err(HwOverflow {
                        what: "transform program",
                        slot,
                    });
                }
                mem.program[slot as usize * 4 + c as usize] = w;
                if c == 3 {
                    self.program_load += 1;
                }
            }
            _ if (TRANSFORM_CONSTANT..TRANSFORM_CONSTANT + 0x80).contains(&m) => {
                let c = ((m - TRANSFORM_CONSTANT) / 4) % 4;
                let slot = self.constant_load;
                if slot as usize >= CONSTANTS {
                    return Err(HwOverflow {
                        what: "transform constant",
                        slot,
                    });
                }
                mem.constants[slot as usize * 4 + c as usize] = w;
                if c == 3 {
                    self.constant_load += 1;
                }
            }
            STIPPLE_CONTROL => self.stipple_control = Some(w),
            _ if (STIPPLE_PATTERN..STIPPLE_PATTERN + 0x80).contains(&m) => {
                self.stipple_pattern[((m - STIPPLE_PATTERN) / 4) as usize] = w;
            }
            BREAK_VERTEX_BUFFER_CACHE => {}
            _ if (VERTEX_DATA_ARRAY_OFFSET..VERTEX_DATA_ARRAY_OFFSET + 0x40).contains(&m) => {
                self.vertex_offset[((m - VERTEX_DATA_ARRAY_OFFSET) / 4) as usize] = w;
            }
            _ if (VERTEX_DATA_ARRAY_FORMAT..VERTEX_DATA_ARRAY_FORMAT + 0x40).contains(&m) => {
                self.vertex_format[((m - VERTEX_DATA_ARRAY_FORMAT) / 4) as usize] = w;
            }
            ZPASS_PIXEL_COUNT_ENABLE => self.zpass_count_enable = w != 0,
            _ if (EYE_VECTOR..EYE_VECTOR + 12).contains(&m) => {
                self.eye_vector[((m - EYE_VECTOR) / 4) as usize] = f;
            }
            ZSTENCIL_CLEAR_VALUE => self.zstencil_clear_value = w,
            COLOR_CLEAR_VALUE => self.color_clear_value = w,
            EXECUTION_MODE => self.execution_mode = w,
            CXT_WRITE_EN => self.cxt_write_enable = w,
            PROGRAM_LOAD => self.program_load = w,
            PROGRAM_START => self.program_start = Some(w),
            CONSTANT_LOAD => self.constant_load = w,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mems() -> (Box<[u32; CONSTANTS * 4]>, Box<[u32; PROGRAM_SLOTS * 4]>) {
        (
            Box::new([0; CONSTANTS * 4]),
            Box::new([0; PROGRAM_SLOTS * 4]),
        )
    }

    #[test]
    fn constant_and_program_cursors_advance_per_register() {
        let (mut c, mut p) = mems();
        let mut m = Memories {
            constants: &mut c,
            program: &mut p,
        };
        let mut hw = HwState::default();
        assert!(hw.apply(method::CONSTANT_LOAD, 2, &mut m).unwrap());
        for i in 0..8 {
            hw.apply(method::TRANSFORM_CONSTANT + 4 * i, 100 + i, &mut m)
                .unwrap();
        }
        assert_eq!(hw.constant_load, 4);
        assert_eq!(
            &m.constants[8..16],
            &[100, 101, 102, 103, 104, 105, 106, 107]
        );
        hw.apply(method::PROGRAM_LOAD, 135, &mut m).unwrap();
        for i in 0..4 {
            hw.apply(method::TRANSFORM_PROGRAM + 4 * i, 7 + i, &mut m)
                .unwrap();
        }
        assert_eq!(&m.program[135 * 4..], &[7, 8, 9, 10]);
        let err = hw.apply(method::TRANSFORM_PROGRAM, 0, &mut m).unwrap_err();
        assert_eq!(err.slot, 136);
    }

    #[test]
    fn viewport_registers_mirror_into_constants_58_and_59() {
        let (mut c, mut p) = mems();
        let mut m = Memories {
            constants: &mut c,
            program: &mut p,
        };
        let mut hw = HwState::default();
        hw.apply(method::VIEWPORT_SCALE + 4, 2.0f32.to_bits(), &mut m)
            .unwrap();
        hw.apply(method::VIEWPORT_OFFSET, 0.5f32.to_bits(), &mut m)
            .unwrap();
        assert_eq!(hw.viewport_scale[1], 2.0);
        assert_eq!(m.constants[58 * 4 + 1], 2.0f32.to_bits());
        assert_eq!(m.constants[59 * 4], 0.5f32.to_bits());
        assert!(!hw.apply(0x17FC, 0, &mut m).unwrap());
    }

    #[test]
    fn eye_vector_lands_in_its_three_registers() {
        let (mut c, mut p) = mems();
        let mut m = Memories {
            constants: &mut c,
            program: &mut p,
        };
        let mut hw = HwState::default();
        for (i, v) in [0.5f32, -1.0, 2.0].iter().enumerate() {
            assert!(
                hw.apply(method::EYE_VECTOR + 4 * i as u32, v.to_bits(), &mut m)
                    .unwrap()
            );
        }
        assert_eq!(hw.eye_vector, [0.5, -1.0, 2.0]);
        // The method after it is not part of it.
        assert!(!hw.apply(method::EYE_VECTOR + 12, 0, &mut m).unwrap());
    }

    #[test]
    fn vertex_array_formats_and_offsets_land_per_register() {
        let (mut c, mut p) = mems();
        let mut m = Memories {
            constants: &mut c,
            program: &mut p,
        };
        let mut hw = HwState::default();
        assert_eq!(hw.vertex_format, [VERTEX_FORMAT_DISABLED; 16]);
        // Register 0: float3, stride 24; register 3: D3DCOLOR.
        assert!(
            hw.apply(method::VERTEX_DATA_ARRAY_FORMAT, 24 << 8 | 0x32, &mut m)
                .unwrap()
        );
        assert!(
            hw.apply(
                method::VERTEX_DATA_ARRAY_FORMAT + 12,
                24 << 8 | 0x40,
                &mut m
            )
            .unwrap()
        );
        assert!(
            hw.apply(method::VERTEX_DATA_ARRAY_OFFSET + 60, 0x1234, &mut m)
                .unwrap()
        );
        assert_eq!(hw.vertex_format[0], 0x1832);
        assert_eq!(hw.vertex_format[3], 0x1840);
        assert_eq!(hw.vertex_format[1], VERTEX_FORMAT_DISABLED);
        assert_eq!(hw.vertex_offset[15], 0x1234);
        // The ranges end where the next method begins.
        assert!(
            !hw.apply(method::VERTEX_DATA_ARRAY_FORMAT + 0x40, 0, &mut m)
                .unwrap()
        );
    }
}
