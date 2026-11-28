//! An object part (`obje`, `0x17ab44`): the part switch's own work around
//! its seven leaves.
//!
//! With `S` = the switch's entry `ESP - 0x110` and `P = S + 0x34` the
//! placement record:
//!
//! 1. `0xb7930(tag, object, effect + 0x4c)` (`ESI` = `P`) fills `P`;
//! 2. `0xa7640` (`EAX` = `P`): 0 returns;
//! 3. the arm copies the position, the direction and the entry `EAX`'s
//!    vector to `S + 0x50..0x74`;
//! 4. `0x177d10` places the object (it draws word 0 of the shared stream);
//! 5. the arm adds the effect's `+0x30` vector to `S + 0x74..0x80`;
//! 6. `0x177f60` sets its velocity (it draws too);
//! 7. `0xb7b40(P)` creates the object, `-1` returns; then `0xbb950(1, 0)`
//!    and `0xa7870(object)`.
//!
//! Every draw is in a leaf, so Rust making the calls in this order with
//! these registers keeps the stream's order; [`sums`] is step 5.

use crate::sound::Unmasked;

/// The leaves, in call order.
pub const INIT: u32 = 0x000b_7930;
pub const CHECK: u32 = 0x000a_7640;
pub const PLACE: u32 = 0x0017_7d10;
pub const VELOCITY: u32 = 0x0017_7f60;
pub const NEW: u32 = 0x000b_7b40;
pub const AFTER_NEW: u32 = 0x000b_b950;
pub const ATTACH: u32 = 0x000a_7870;
/// Where the switch calls the two checked leaves from.
pub const PLACE_RETURN: u32 = 0x0017_abe7;
pub const VELOCITY_RETURN: u32 = 0x0017_ac3f;

/// Offsets from `S`.
pub const PLACEMENT: u32 = 0x34;
pub const COPIES: u32 = 0x50;
pub const SUMS: u32 = 0x74;
pub const VELOCITY_OUT: u32 = 0x80;
pub const PLACE_OUT: u32 = 0xf8;

/// MXCSR's exception masks. The arm answers only with all six set: then
/// the step 5 adds cannot fault mid-answer.
pub const MASKED: u32 = 0x1f80;

/// The shared stream's pointer, passed to both drawing leaves.
pub const RNG: u32 = 0x004e_7408;

/// The part record's words the arm passes on: `+0x14`, `+0x18`, `+0x1c`,
/// `+0x20`, `+0x24`, `+0x30`, `+0x34`.
pub const PART_WORDS: [u32; 7] = [0x14, 0x18, 0x1c, 0x20, 0x24, 0x30, 0x34];

/// Everything the arm reads at the switch's entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// `S`.
    pub frame: u32,
    pub effect: u32,
    pub tag: u32,
    /// `[effect + 0x48]`.
    pub object: u32,
    /// `EAX` at entry, the third vector's pointer, and the vectors.
    pub source: u32,
    pub position: [u32; 3],
    pub direction_ptr: u32,
    pub direction: [u32; 3],
    pub source_vec: [u32; 3],
    /// `[0x4e7408]`.
    pub rng: u32,
    pub part: [u32; 7],
    pub mxcsr: u32,
}

impl Inputs {
    fn p(&self, offset: u32) -> u32 {
        self.part[PART_WORDS.iter().position(|&o| o == offset).expect("a part word")]
    }

    pub fn placement(&self) -> u32 {
        self.frame.wrapping_add(PLACEMENT)
    }

    /// Step 1's arguments and its registers (`EAX`, `ECX`, `EDX`, `EBX`,
    /// `ESI`, `EDI`).
    pub fn init_args(&self) -> [u32; 3] {
        [self.tag, self.object, self.effect.wrapping_add(0x4c)]
    }
    pub fn init_regs(&self) -> [u32; 6] {
        [self.object, self.tag, self.effect.wrapping_add(0x4c), self.effect, self.placement(), self.source]
    }

    /// Step 3: the nine words at `S + 0x50`.
    pub fn copies(&self) -> [u32; 9] {
        let (p, d, s) = (self.position, self.direction, self.source_vec);
        [p[0], p[1], p[2], d[0], d[1], d[2], s[0], s[1], s[2]]
    }

    /// Step 4's arguments and registers.
    pub fn place_args(&self) -> [u32; 9] {
        [
            self.rng,
            self.direction_ptr,
            self.frame.wrapping_add(PLACE_OUT),
            self.frame.wrapping_add(SUMS),
            self.p(0x14),
            self.p(0x18),
            self.p(0x1c),
            self.p(0x30),
            self.p(0x34),
        ]
    }
    pub fn place_regs(&self) -> [u32; 6] {
        [
            self.effect,
            self.frame.wrapping_add(PLACE_OUT),
            self.frame.wrapping_add(SUMS),
            self.effect,
            self.rng,
            self.source,
        ]
    }

    /// Step 6's arguments and registers.
    pub fn velocity_args(&self) -> [u32; 4] {
        [self.rng, self.frame.wrapping_add(VELOCITY_OUT), self.p(0x20), self.p(0x24)]
    }
    pub fn velocity_regs(&self) -> [u32; 6] {
        [
            self.p(0x20),
            self.frame.wrapping_add(VELOCITY_OUT),
            self.p(0x24),
            self.effect,
            self.p(0x30),
            self.p(0x34),
        ]
    }
}

/// Step 5: `effect[0x30 + 4n] + S[0x74 + 4n]` in SSE, as `addss` does it.
pub fn sums(mxcsr: u32, offset: [u32; 3], placed: [u32; 3]) -> Result<[u32; 3], Unmasked> {
    let mut out = [0; 3];
    for n in 0..3 {
        let mut m = mxcsr;
        out[n] = simd::addss(&mut m, u128::from(offset[n]), placed[n]).map_err(|_| Unmasked)? as u32;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn inputs() -> Inputs {
        Inputs {
            frame: 0x002f_fef0,
            effect: 0x0061_0000,
            tag: 0xe100_0002,
            object: 0x0005_0001,
            source: 0x0062_0000,
            position: [1, 2, 3],
            direction_ptr: 0x0063_0000,
            direction: [4, 5, 6],
            source_vec: [7, 8, 9],
            rng: 0x0070_0000,
            part: [0x14, 0x18, 0x1c, 0x20, 0x24, 0x30, 0x34],
            mxcsr: 0x1f80,
        }
    }

    /// The leaves' arguments and registers as the disassembly of
    /// `0x17ab44..0x17ac5e` pushes and sets them.
    #[test]
    fn the_leaf_calls_are_the_arms() {
        let i = inputs();
        let s = i.frame;
        assert_eq!(i.init_args(), [i.tag, i.object, 0x0061_004c]);
        assert_eq!(i.place_args(), [0x0070_0000, 0x0063_0000, s + 0xf8, s + 0x74, 0x14, 0x18, 0x1c, 0x30, 0x34]);
        assert_eq!(i.place_regs(), [i.effect, s + 0xf8, s + 0x74, i.effect, i.rng, i.source]);
        assert_eq!(i.velocity_args(), [i.rng, s + 0x80, 0x20, 0x24]);
        assert_eq!(i.velocity_regs(), [0x20, s + 0x80, 0x24, i.effect, 0x30, 0x34]);
        assert_eq!(i.copies(), [1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn the_sums_are_sse_adds_with_retails_operand_order() {
        let one = 1.0f32.to_bits();
        assert_eq!(sums(0x1f80, [one; 3], [2.0f32.to_bits(); 3]), Ok([3.0f32.to_bits(); 3]));
        // Two NaNs: addss keeps the first operand's (the effect's) payload.
        let (a, b) = (0x7fc0_0001, 0x7fc0_0002);
        assert_eq!(sums(0x1f80, [a, one, one], [b, one, one]).unwrap()[0], a);
        // An unmasked exception is refused.
        assert_eq!(sums(0x1f80 & !0x0080, [0x7f80_0001, one, one], [one; 3]), Err(Unmasked));
    }
}
