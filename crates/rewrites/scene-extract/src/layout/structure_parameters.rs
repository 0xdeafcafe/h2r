//! The active structure's parameter block, which the geometry draw's
//! prologue `0x0004d640` uploads for a shader with flag bit 0
//! (`0x0003c650`, `0x0003d000`): the arithmetic is
//! `engine::render_loop::structure_parameters`, run here over guest
//! memory under the game's x87 control word and MXCSR (docs/lifter/FLOAT.md:
//! `0x027f`, `0x1f80`). The host takes a part that uploads them only where
//! the device and the engine already hold every word, so a word computed
//! otherwise than the guest's declines; it does not draw wrong.

use engine::memory::{GuestMemory, MemFault};
use engine::render_loop::structure_parameters as upload;
use frame_scene::StructureParameters;

use crate::GuestRead;

/// The game's x87 control word and MXCSR.
pub const FCW: u16 = 0x027f;
pub const MXCSR: u32 = 0x1f80;

/// Guest memory read through [`GuestRead`] (aligned words), read only.
struct Words<'a>(&'a dyn GuestRead);

impl GuestMemory for Words<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let fault = MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        };
        for (i, byte) in buf.iter_mut().enumerate() {
            let at = addr.checked_add(i as u32).ok_or(fault)?;
            let word = self.0.read_u32(at & !3).ok_or(fault)?;
            *byte = word.to_le_bytes()[(at & 3) as usize];
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
    }
}

/// Whether the active structure has a parameter block (`+0x214` above
/// 0), so a flagged shader's prologue uploads it; `None` when a word
/// does not read.
pub fn uploads(mem: &dyn GuestRead) -> Option<bool> {
    upload::block(&Words(mem)).ok().map(|b| b.is_some())
}

/// What the prologue uploads for the active structure's block; `None`
/// when it has none, a word does not read or the arithmetic stops.
pub fn read(mem: &dyn GuestRead) -> Option<StructureParameters> {
    let block = upload::block(&Words(mem)).ok()??;
    of(mem, block)
}

/// What `0x0003bf00` uploads for the parameter block `block` (the water
/// end's ocean passes its definition); `None` when a word does not read
/// or the arithmetic stops.
pub fn of(mem: &dyn GuestRead, block: u32) -> Option<StructureParameters> {
    let u = upload::upload(&Words(mem), block, FCW, MXCSR).ok()?;
    Some(StructureParameters {
        bump_env: u.bump_env,
        constants: u.constants,
        fog_colour: u.fog_colour,
        scratch: u.scratch,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::Mem;

    const BSP: u32 = 0x0061_0000;
    pub(crate) const BLOCK: u32 = 0x0060_0000;
    const UP: u32 = 0x0062_0000;

    /// Synthetic words for every input of the upload: the view along y,
    /// up along z, the block's point at `+0x34` (3, 4, 5) and its fog
    /// colour (1, 0.5, 0).
    pub(crate) fn guest() -> Mem {
        let mut m = Mem::default();
        m.w(upload::ACTIVE_BSP, BSP);
        m.w(BSP + upload::BSP_PARAMETER_COUNT, 1);
        m.w(BSP + upload::BSP_PARAMETERS, BLOCK);
        for (at, v) in [
            (0x0045_dbc0, 1.0),
            (0x0045_dbdc, 1e-4),
            (0x0045_dbbc, 0.5),
            (0x0045_dbcc, 0.25),
            (0x0045_e458, 2.0),
            (0x0045_dc38, 255.0),
            (0x004b_9dac, 0.0),
            (0x004b_9db0, 1.0),
            (UP, 0.0),
            (UP + 4, 0.0),
            (UP + 8, 1.0),
        ] {
            m.f(at, v);
        }
        let range = 0.5f64.to_bits();
        m.w(0x0045_e450, range as u32);
        m.w(0x0045_e454, (range >> 32) as u32);
        m.w(0x0046_87b0, UP);
        for offset in (0..0xa8).step_by(4) {
            m.w(BLOCK + offset, 0);
        }
        for (offset, v) in [
            (0x34, 3.0),
            (0x38, 4.0),
            (0x3c, 5.0),
            (0x58, 1.0),
            (0x5c, 0.5),
        ] {
            m.f(BLOCK + offset, v);
        }
        m
    }

    /// Read through extraction's word reads, the upload is the engine's:
    /// `c[25]` the block's point and one, the fog colour packed; none
    /// without a block, and none when a word it reads does not read.
    #[test]
    fn the_views_upload_is_read_from_the_active_structures_block() {
        let mut m = guest();
        assert_eq!(uploads(&m), Some(true));
        let p = read(&m).expect("the upload computes");
        assert_eq!(p.constants[4], [3.0f32, 4.0, 5.0, 1.0].map(f32::to_bits));
        assert_eq!(p.fog_colour, 0x00ff_8000);
        assert_eq!(p.scratch[0], 1f32.to_bits());
        m.0.remove(&(BLOCK + 0x70));
        assert_eq!(read(&m), None, "a word that does not read");
        m.w(BSP + upload::BSP_PARAMETER_COUNT, 0);
        assert_eq!(uploads(&m), Some(false));
        assert_eq!(read(&m), None, "no block, no upload");
        m.0.remove(&upload::ACTIVE_BSP);
        assert_eq!(uploads(&m), None);
    }
}
