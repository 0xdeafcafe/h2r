//! A decal part (`deca`, `0x17aa5c`): the switch's own work around its
//! three leaves. `S` is the entry `ESP - 0x110`; `R` the stream pointer
//! `[0x4e7408]`, read first.
//!
//! 1. The part's byte `+6`, shifted right 4, bit 0 clear (`gated`):
//!    `0x172750` (`EAX` 1, `EDI` the position pointer, `XMM0`
//!    `[0x45dc08]`); `AL` 0 draws from `R+4` ([`crate::random::unit`]) and
//!    returns, `EAX` the seed's top half, unless `[0x45dc20]` is above the
//!    draw.
//! 2. `0x177d10(R+4, direction, S+0xf8, S+0x104, five part words)`, which
//!    draws from `R+4` too.
//! 3. One more draw, `[part+0x28]` to `[part+0x2c]`, stored as an `f32`.
//! 4. `0x17e670(tag, S+0x104, the draw, 0, -1, 0)` with `ECX` = `[0x510c78]`
//!    (`ret 0x18`); its `EAX` returns.
//!
//! Rust's answer makes the leaf calls in this order and the draws between
//! them on a copy of the entry's x87 state, so the stream's order is
//! retail's. The audit is staged ([`Plan`]): at `0x177d10`'s entry the
//! gate's draw and the call; at its return the step 3 draw is predicted;
//! at `0x17e670`'s entry the call and the stream; at the switch's return a
//! part the guest gated out.

use x87::abi::{SW_ES, X87Abi};

use crate::arm::Prediction;
use crate::random::{self, Refused};

pub const NEAR: u32 = 0x0017_2750;
pub const CREATE: u32 = 0x0017_e670;
/// Where the switch calls `0x177d10` and `0x17e670` from.
pub const PLACE_RETURN: u32 = 0x0017_aad9;
pub const CREATE_RETURN: u32 = 0x0017_ab10;
/// `0x172750`'s radius, the gate's limit, the decal system.
pub const RADIUS: u32 = 0x0045_dc08;
pub const LIMIT: u32 = 0x0045_dc20;
pub const DECALS: u32 = 0x0051_0c78;
/// Offsets from `S`.
pub const OUT: u32 = 0xf8;
pub const PLACED: u32 = 0x104;
/// The part words `0x177d10` takes, then the draw's range.
pub const PART_WORDS: [u32; 7] = [0x14, 0x18, 0x1c, 0x30, 0x34, 0x28, 0x2c];

/// Everything the arm reads at the switch's entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// The switch's entry `ESP`.
    pub entry: u32,
    pub effect: u32,
    pub tag: u32,
    /// The part's byte `+6`.
    pub flags: u8,
    /// `R`, and the seed at `R+4`.
    pub rng: u32,
    pub seed: u32,
    pub position_ptr: u32,
    pub direction_ptr: u32,
    /// `EAX`, `ECX`, `EDX` at entry.
    pub source: u32,
    pub ecx: u32,
    pub edx: u32,
    pub part: [u32; 7],
    /// `[0x45dc50]`, `[0x45dc20]`, `[0x45dc08]`.
    pub scale: u32,
    pub limit: u32,
    pub radius: u32,
    pub fpu: X87Abi,
}

impl Inputs {
    fn frame(&self) -> u32 {
        self.entry.wrapping_sub(0x110)
    }
    pub fn seed_at(&self) -> u32 {
        self.rng.wrapping_add(4)
    }
    pub fn gated(&self) -> bool {
        (self.flags >> 4) & 1 == 0
    }
    /// `EDI` from step 1 on: the position pointer when gated, else the
    /// entry `EAX`.
    pub fn edi(&self) -> u32 {
        if self.gated() { self.position_ptr } else { self.source }
    }
    /// The x87 helpers cannot refuse: no exception pending and all six
    /// masked. The arm answers only then.
    pub fn quiet_fpu(&self) -> bool {
        self.fpu.status & SW_ES == 0 && self.fpu.control & 0x3f == 0x3f
    }

    /// Step 1's registers (`EAX`, `ECX`, `EDX`, `EBX`, `ESI`, `EDI`) and
    /// `XMM0`.
    pub fn near_regs(&self) -> [u32; 6] {
        let ecx = (self.ecx & !0xff) | u32::from(self.flags >> 4);
        [1, ecx, self.edx, self.effect, self.rng, self.position_ptr]
    }

    pub fn place_args(&self) -> [u32; 9] {
        let (s, p) = (self.frame(), &self.part);
        [self.seed_at(), self.direction_ptr, s + OUT, s + PLACED, p[0], p[1], p[2], p[3], p[4]]
    }
    pub fn place_regs(&self) -> [u32; 6] {
        let s = self.frame();
        [self.effect, s + PLACED, s + OUT, self.effect, self.seed_at(), self.edi()]
    }

    /// Step 3 on `fpu` from the stream's seed: the new seed and the `f32`.
    pub fn draw(&self, fpu: &mut X87Abi, seed: u32) -> Result<(u32, u32), Refused> {
        let seed = random::range(fpu, seed, self.scale, self.part[5], self.part[6])?;
        Ok((seed, random::store_f32(fpu)?))
    }

    /// Step 1's draw: the new seed and whether the arm goes on.
    pub fn gate(&self, fpu: &mut X87Abi, seed: u32) -> Result<(u32, bool), Refused> {
        let seed = random::unit(fpu, seed, self.scale)?;
        Ok((seed, random::below(fpu, self.limit)?))
    }

    pub fn create_args(&self, draw: u32) -> [u32; 6] {
        [self.tag, self.frame() + PLACED, draw, 0, u32::MAX, 0]
    }
    pub fn create_regs(&self, decals: u32) -> [u32; 6] {
        let s = self.frame();
        [s + PLACED, decals, self.position_ptr, self.effect, self.seed_at(), self.edi()]
    }
}

/// The arm's staged prediction for one decal part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub inputs: Inputs,
    /// The stream's seed at entry.
    pub seed: u32,
    /// `0x177d10` was reached and matched, and whether the gate drew
    /// before it.
    pub placed: bool,
    pub gate_drew: bool,
    /// The step 3 draw from what `0x177d10` left: the seed after it and
    /// the `f32`.
    pub draw: Option<(u32, u32)>,
}

/// The guest at `0x177d10`'s or `0x17e670`'s entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen<const A: usize> {
    pub seed: u32,
    pub args: [u32; A],
    pub regs: [u32; 6],
}

fn diff<const A: usize>(args: [u32; A], regs: [u32; 6], seen: &Seen<A>, base: usize) -> Option<usize> {
    let want = args.iter().chain(&regs);
    let got = seen.args.iter().chain(&seen.regs);
    want.zip(got).position(|(a, b)| a != b).map(|i| base + i)
}

impl Plan {
    pub fn new(inputs: Inputs) -> Plan {
        Plan { inputs, seed: inputs.seed, placed: false, gate_drew: false, draw: None }
    }

    /// At `0x177d10`'s entry: compare, and on a match note the stage.
    pub fn place(&mut self, seen: &Seen<9>) -> Option<usize> {
        let at = self.place_difference(seen);
        if at.is_none() {
            self.placed = true;
            self.gate_drew = seen.seed != self.seed;
        }
        at
    }

    /// At `0x177d10`'s entry. 0: the stream is not where the gate leaves
    /// it (no draw when not gated or when `0x172750` said near; one draw
    /// Rust's gate lets on). 1 to 9 the arguments, 10 to 15 the registers.
    pub fn place_difference(&self, seen: &Seen<9>) -> Option<usize> {
        let i = &self.inputs;
        let drew = i.gated() && seen.seed == random::next(self.seed) && {
            let mut fpu = i.fpu;
            matches!(i.gate(&mut fpu, self.seed), Ok((_, true)))
        };
        if seen.seed != self.seed && !drew {
            return Some(0);
        }
        diff(i.place_args(), i.place_regs(), seen, 1)
    }

    /// At `0x177d10`'s return: the step 3 draw from the seed it left.
    /// The gate's draw, if any, ran first on the same x87 state.
    pub fn predict_draw(&mut self, seed_now: u32) -> Result<(), Refused> {
        let i = self.inputs;
        let mut fpu = i.fpu;
        if self.gate_drew {
            i.gate(&mut fpu, self.seed)?;
        }
        self.draw = Some(i.draw(&mut fpu, seed_now)?);
        Ok(())
    }

    /// At `0x17e670`'s entry. 16 the stream, 17 to 22 the arguments, 23 to
    /// 28 the registers; 29: the draw was never predicted. `decals` is the
    /// guest's `[0x510c78]`.
    pub fn create_difference(&self, seen: &Seen<6>, decals: u32) -> Option<usize> {
        let i = &self.inputs;
        let Some((seed, f)) = self.draw else { return Some(29) };
        if seen.seed != seed {
            return Some(16);
        }
        diff(i.create_args(f), i.create_regs(decals), seen, 17)
    }

    /// At the switch's return, for a part that never reached `0x177d10`:
    /// the guest gated it out, so `0x172750` was near-false and the gate's
    /// draw stopped it. 30: Rust's gate would have gone on, or the stream
    /// or `EAX` differ.
    pub fn gated_out_difference(&self, seed_now: u32, eax: u32) -> Option<usize> {
        let i = &self.inputs;
        let mut fpu = i.fpu;
        let stop = i.gated() && matches!(i.gate(&mut fpu, self.seed), Ok((_, false)));
        let seed = random::next(self.seed);
        (!(stop && seed_now == seed && eax == seed >> 16)).then_some(30)
    }
}

impl Prediction for Plan {
    fn first_difference(&self, seen: &Plan) -> Option<usize> {
        (self != seen).then_some(0)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn inputs() -> Inputs {
        Inputs {
            entry: 0x0030_0000,
            effect: 0x0061_0000,
            tag: 0xe100_0005,
            flags: 0,
            rng: 0x0070_0000,
            seed: 0,
            position_ptr: 0x0063_0000,
            direction_ptr: 0x0063_0010,
            source: 0x0062_0000,
            ecx: 0x1234_5678,
            edx: 0x0000_0099,
            part: [0x14, 0x18, 0x1c, 0x30, 0x34, 0, 1.0f32.to_bits()],
            scale: 0x3780_0000,
            limit: 0.5f32.to_bits(),
            radius: 4.0f32.to_bits(),
            fpu: X87Abi { control: 0x027f, ..X87Abi::default() },
        }
    }

    /// The leaf calls as `0x17aa5c..0x17ab0b` sets them up.
    #[test]
    fn the_leaf_calls_are_the_arms() {
        let i = inputs();
        let s = i.entry - 0x110;
        assert_eq!(i.near_regs(), [1, 0x1234_5600, 0x99, i.effect, i.rng, i.position_ptr]);
        assert_eq!(i.place_args(), [0x0070_0004, 0x0063_0010, s + 0xf8, s + 0x104, 0x14, 0x18, 0x1c, 0x30, 0x34]);
        assert_eq!(i.place_regs(), [i.effect, s + 0x104, s + 0xf8, i.effect, 0x0070_0004, i.position_ptr]);
        assert_eq!(i.create_args(7), [i.tag, s + 0x104, 7, 0, u32::MAX, 0]);
        assert_eq!(i.create_regs(0x0080_0000), [s + 0x104, 0x0080_0000, i.position_ptr, i.effect, 0x0070_0004, i.position_ptr]);
        let ungated = Inputs { flags: 0x10, ..i };
        assert!(!ungated.gated());
        assert_eq!((ungated.edi(), ungated.near_regs()[1] & 0xff), (i.source, 1));
    }

    /// Each stage accepts what the guest's arm leaves and names a stream
    /// the gate could not have left.
    #[test]
    fn the_stages_follow_the_stream() {
        let i = inputs();
        let mut plan = Plan::new(i);
        let seen = |seed| Seen { seed, args: i.place_args(), regs: i.place_regs() };
        // Near (no draw), or far and the first draw from 0 (about 0.24)
        // under the limit 0.5: both reach 0x177d10.
        assert_eq!(plan.place_difference(&seen(0)), None);
        assert_eq!(plan.place(&seen(random::next(0))), None);
        assert!(plan.placed && plan.gate_drew);
        assert_eq!(plan.place_difference(&seen(5)), Some(0));
        let strict = Plan::new(Inputs { limit: 0.1f32.to_bits(), ..i });
        assert_eq!(strict.place_difference(&seen(random::next(0))), Some(0), "the gate stops that draw");
        assert_eq!(strict.gated_out_difference(random::next(0), random::next(0) >> 16), None);
        assert_eq!(plan.gated_out_difference(random::next(0), random::next(0) >> 16), Some(30));
        // 0x177d10 drew once more; the step 3 draw follows from there.
        let after = random::next(random::next(0));
        plan.predict_draw(after).unwrap();
        let (seed, f) = plan.draw.unwrap();
        assert_eq!(seed, random::next(after));
        assert_eq!(f32::from_bits(f), (seed >> 16) as f32 / 65536.0);
        let create = Seen { seed, args: i.create_args(f), regs: i.create_regs(9) };
        assert_eq!(plan.create_difference(&create, 9), None);
        assert_eq!(plan.create_difference(&Seen { seed: 1, ..create }, 9), Some(16));
    }
}
