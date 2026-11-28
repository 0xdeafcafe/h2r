//! A `coln` part (`0x17a94f`): one draw from the stream's word `+4`,
//! `0x259d0([part+0x28], [part+0x2c])` with `ECX` = `[0x4e7408] + 4`, the
//! value dropped (`fstp st(0)`). Nothing else changes.
//!
//! Rust makes the draw itself on the guest's x87 state ([`crate::random`])
//! and returns with the registers the arm leaves: `EAX` = the new seed's
//! top half, `ECX` the stream word's address, `EDX` the part's `+0x2c`.

use x87::abi::X87Abi;

use crate::random::{self, Refused};

/// Everything the arm reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// The switch's entry `ESP`.
    pub entry: u32,
    /// `[0x4e7408] + 4` and the seed there.
    pub seed_at: u32,
    pub seed: u32,
    /// The part's `+0x28` and `+0x2c`.
    pub lo: u32,
    pub hi: u32,
    /// `[0x45dc50]`.
    pub scale: u32,
    pub fpu: X87Abi,
}

/// What the arm leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub entry: u32,
    pub seed_at: u32,
    pub seed: u32,
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub fpu: X87Abi,
}

impl Call {
    pub fn predict(i: &Inputs) -> Result<Call, Refused> {
        let mut fpu = i.fpu;
        let seed = random::range(&mut fpu, i.seed, i.scale, i.lo, i.hi)?;
        random::drop_top(&mut fpu)?;
        Ok(Call { entry: i.entry, seed_at: i.seed_at, seed, eax: seed >> 16, ecx: i.seed_at, edx: i.hi, fpu })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_draw_is_one_step_and_the_value_is_dropped() {
        let fpu = X87Abi { control: 0x027f, ..X87Abi::default() };
        let i = Inputs {
            entry: 0x0030_0000,
            seed_at: 0x0070_0004,
            seed: 7,
            lo: 0,
            hi: 1.0f32.to_bits(),
            scale: 0x3780_0000,
            fpu,
        };
        let c = Call::predict(&i).unwrap();
        assert_eq!((c.seed, c.eax, c.ecx, c.edx), (random::next(7), random::next(7) >> 16, 0x0070_0004, i.hi));
        assert_eq!((c.fpu.status & x87::abi::SW_TOP, c.fpu.tag), (fpu.status & x87::abi::SW_TOP, fpu.tag));
        assert_ne!(c.fpu.st, fpu.st, "the freed registers hold the draw, as retail's do");
    }
}
