//! The second fog's screen fans (`0x00025dc0`,
//! docs/graphics/retire-d3d-fog-fans.md): which fans it draws, as a pure
//! function of guest memory and its two flag bytes.
//!
//! (Code, `h2 disasm 0x25dc0 40`, `0x26880 240`, `0x26e50 410`,
//! `0x276d0 160`, `0x34420 215`.)
//!
//! `0x00025dc0(cl, dl)` draws only when the fog's tag datum
//! `[0x004857b8]` is neither -1 nor 0 and `[0x004857d8]` is above
//! `[0x0045dbd8]` (`comiss; jbe`). Then it stores `cl` and `dl` at
//! `0x005093fd` and `0x005093fe` and draws through the fan `0x00034420`:
//!
//! | Flags | Fans (setup, attribute callback) |
//! |---|---|
//! | `cl` set | [`Fans::Layers`]: `0x00026880`, `0x00026ca0`, into target `0x13`; then `0x00026e50`, `0x00027520` |
//! | `cl` clear, `dl` set | [`Fans::Composite`]: `0x000276d0`, `0x00027960`, target 3 into the view's |
//! | neither | none |
//!
//! Every path ends with the depth clip control `0x00041d78` set to 0 and
//! the words `0x004b8344` and `0x004075e0` zeroed.

use crate::light_constants::{Error, Sse};
use crate::memory::GuestMemory;

/// `0x00025dc0`.
pub const FOG_FANS: u32 = 0x0002_5dc0;
/// The fan every draw goes through.
pub const FAN: u32 = 0x0003_4420;

/// The fans' setups (handed to the fan in EAX) and attribute callbacks.
pub mod setup {
    /// The fog's four layers into target `0x13`.
    pub const LAYERS: u32 = 0x0002_6880;
    pub const LAYERS_CORNERS: u32 = 0x0002_6ca0;
    /// Target `0x13` over the view, after [`LAYERS`].
    pub const LAYERS_COMPOSITE: u32 = 0x0002_6e50;
    pub const LAYERS_COMPOSITE_CORNERS: u32 = 0x0002_7520;
    /// Target 3 over the view (`dl` alone).
    pub const COMPOSITE: u32 = 0x0002_76d0;
    pub const COMPOSITE_CORNERS: u32 = 0x0002_7960;
}

/// Guest globals and image constants `0x00025dc0` reads.
pub mod global {
    /// The fog's tag datum: -1 or 0, no fans.
    pub const DATUM: u32 = 0x0048_57b8;
    /// The fog's amount (`f32`), drawn only above [`ZERO`].
    pub const AMOUNT: u32 = 0x0048_57d8;
    /// 0.0.
    pub const ZERO: u32 = 0x0045_dbd8;
    /// Where the flags are kept for the setups (`cl`, `dl`).
    pub const FLAG_CL: u32 = 0x0050_93fd;
    pub const FLAG_DL: u32 = 0x0050_93fe;
}

/// What `0x00025dc0` draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fans {
    /// No fan: the gate stopped it, or neither flag is set.
    None,
    /// `cl`: the layers into target `0x13`, then their composite.
    Layers,
    /// `dl` alone: target 3's composite.
    Composite,
}

impl Fans {
    /// The cause an unported draw panics with.
    pub const fn cause(self) -> &'static str {
        match self {
            Fans::None => "none",
            Fans::Layers => "layers (0x00026880 into target 0x13, then 0x00026e50)",
            Fans::Composite => "composite (0x000276d0, target 3)",
        }
    }
}

/// The gate: the datum neither -1 nor 0 and the amount above 0, under
/// the call's MXCSR.
pub fn gate(mem: &dyn GuestMemory, mxcsr: u32) -> Result<bool, Error> {
    let datum = mem.u32(global::DATUM)?;
    if datum == u32::MAX || datum == 0 {
        return Ok(false);
    }
    Sse { mxcsr }.above(mem.u32(global::AMOUNT)?, mem.u32(global::ZERO)?)
}

/// The fans `0x00025dc0` draws for the flag bytes `cl` and `dl`.
pub fn fans(mem: &dyn GuestMemory, mxcsr: u32, cl: u8, dl: u8) -> Result<Fans, Error> {
    if !gate(mem, mxcsr)? {
        return Ok(Fans::None);
    }
    Ok(match (cl, dl) {
        (0, 0) => Fans::None,
        (0, _) => Fans::Composite,
        _ => Fans::Layers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;

    const MXCSR: u32 = 0x1f80;

    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        m.set_u32(global::DATUM, 0xe1a3_0012).unwrap();
        m.set_f32(global::AMOUNT, 0.5).unwrap();
        m.set_f32(global::ZERO, 0.0).unwrap();
        m
    }

    #[test]
    fn cl_draws_the_layers_and_dl_alone_the_composite() {
        let m = mem();
        assert_eq!(fans(&m, MXCSR, 1, 0).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR, 0, 1).unwrap(), Fans::Composite);
        assert_eq!(fans(&m, MXCSR, 0, 0).unwrap(), Fans::None);
    }

    #[test]
    fn no_datum_or_no_amount_draws_nothing() {
        for datum in [0, u32::MAX] {
            let mut m = mem();
            m.set_u32(global::DATUM, datum).unwrap();
            assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::None, "{datum:#x}");
        }
        for amount in [0.0f32, -1.0, f32::NAN] {
            let mut m = mem();
            m.set_f32(global::AMOUNT, amount).unwrap();
            assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::None, "{amount}");
        }
    }

    #[test]
    fn a_denormal_amount_draws() {
        // The Pentium III has no DAZ (`simd`), so the smallest denormal is
        // above 0.
        let mut m = mem();
        m.set_u32(global::AMOUNT, 1).unwrap();
        assert_eq!(fans(&m, MXCSR, 1, 0).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR | 0x0040, 1, 0).unwrap(), Fans::Layers);
    }
}
