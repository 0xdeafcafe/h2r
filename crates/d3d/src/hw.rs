//! NV2A register state that D3D's internal fragments push and the device
//! keeps: viewport scale and offset, depth clip range, `SET_CONTROL0`,
//! transform-constant and transform-program loads, and the 32-word block
//! at `0x1480`.
//!
//! These methods reach the push buffer from D3D code the game calls directly
//! (the viewport/transform fragments `0x3f6190`, `0x3f6390`, `0x3f6f40` and
//! the ramp `0x3f8cc0`) and from the device's own copies of them (see
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
    /// The 32-word block `0x3f8cc0` pushes (`0x1480..0x14FC`).
    pub const BLOCK_1480: u32 = 0x1480;
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
    /// The 32 words at `0x1480` (`0x3f8cc0`, also inline at `0x1e370`).
    pub block_1480: [u32; 32],
}

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
            block_1480: [0; 32],
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
            _ if (BLOCK_1480..BLOCK_1480 + 0x80).contains(&m) => {
                self.block_1480[((m - BLOCK_1480) / 4) as usize] = w;
            }
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
}
