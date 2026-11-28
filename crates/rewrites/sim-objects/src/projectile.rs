//! The projectile update `f8de0` and the projectile step's timers `f8eb0`
//! (slice 11 of docs/engine/sim-rewrite-objects.md) over typed views: the
//! inputs are the projectile's body and definition fields, the outputs are
//! writes to the body. No registers, no `runtime`; the guest calls the
//! update makes (the early mover's displacement `109a00`, the step) are
//! closures the caller supplies. The register-level bodies in
//! `engine::sim_objects::{projectile, projectile_step}` are what these are
//! tested against (`engine::sim_objects::projectile_tests`).
//!
//! Float arithmetic is `f32` adds in retail's operand order, which is
//! `addss` on every host this runs on, so results are bit-identical.

use guest_layout::{Fault, Mem, MemMut};

guest_layout::layout! {
    /// A projectile's body (`proj`), the fields the update and the step's
    /// timers read.
    pub struct Body: 0x1b0 {
        /// The velocity, world units a second.
        VELOCITY velocity @ 0x88: [f32; 3],
        /// Bit 0 is the latch the update tests with [`flags::EARLY_MOVER_OFF`].
        LATCH latch @ 0xc1: u8,
        /// The attachment handle slot (`fd410` answers it; -1 none).
        ATTACHMENT attachment @ 0x14c: u32,
        /// [`flags`].
        FLAGS flags @ 0x12c: u32,
        /// The state; the arming path raises it to at least 1.
        STATE state @ 0x130: i16,
        /// The arming accumulator, raised by [`Body::ARMING_RATE`] a step.
        ARMING arming @ 0x158: f32,
        ARMING_RATE arming_rate @ 0x15c: f32,
        /// The age timer, raised by [`Body::AGE_STEP`] a step.
        AGE age @ 0x160: f32,
        AGE_STEP age_step @ 0x164: f32,
        /// A step counter (wraps).
        STEPS steps @ 0x1aa: u16,
    }
}

guest_layout::layout! {
    /// A projectile's definition, the field the timers read.
    pub struct Def: 0xc2 {
        /// The projectile kind that picks which flag arms it.
        KIND kind @ 0xc0: i16,
    }
}

/// Bits of [`Body::FLAGS`].
pub mod flags {
    /// The attachment was already handled (`f8eb0` skips `fd410`).
    pub const NO_ATTACHMENT: u32 = 1 << 1;
    pub const KIND_1: u32 = 1 << 2;
    pub const ARMING_ON: u32 = 1 << 3;
    pub const KIND_2: u32 = 1 << 4;
    /// Set once the arming accumulator runs.
    pub const ARMING: u32 = 1 << 5;
    pub const KIND_3: u32 = 1 << 11;
    /// With the latch, the update does not ask the early mover.
    pub const MOVER_SKIP: u32 = 1 << 14;
    /// Passed to the early mover as its flag.
    pub const MOVER_FLAG: u32 = 1 << 15;
}

/// A velocity or displacement.
pub type Vec3 = [f32; 3];

/// Whether the update asks the early mover for a displacement, and with
/// which flag: not when the latch is set and [`flags::MOVER_SKIP`] clear.
pub fn mover_request(latch: u8, body_flags: u32) -> Option<bool> {
    if latch & 1 != 0 && body_flags & flags::MOVER_SKIP == 0 {
        return None;
    }
    Some(body_flags & flags::MOVER_FLAG != 0)
}

/// `f8de0`. `mover(flag)` is `109a00`: the displacement when it answered
/// non-zero. `step(velocity)` is `f8eb0` on a copy of the velocity taken
/// before the mover is asked. The displacement is added to the body's
/// velocity as the step left it.
pub fn update<M: Mem + MemMut>(
    m: &mut M,
    body: Body,
    mut mover: impl FnMut(&mut M, bool) -> Result<Option<Vec3>, Fault>,
    mut step: impl FnMut(&mut M, Vec3) -> Result<(), Fault>,
) -> Result<(), Fault> {
    let velocity = body.velocity(m)?;
    let latch = body.latch(m)?;
    let body_flags = body.flags(m)?;
    let displacement = match mover_request(latch, body_flags) {
        Some(flag) => mover(m, flag)?,
        None => None,
    };
    step(m, velocity)?;
    if let Some(d) = displacement {
        let mut v = body.velocity(m)?;
        for (v, d) in v.iter_mut().zip(d) {
            *v += d;
        }
        body.set(m, Body::VELOCITY, v)?;
    }
    Ok(())
}

/// Whether `f8eb0` runs the attachment calls before the timers.
pub fn wants_attachment(body_flags: u32) -> bool {
    body_flags & flags::NO_ATTACHMENT == 0
}

/// The flag the definition's kind selects: kinds 1, 2 and 3 their own
/// bit, every other kind always.
pub fn kind_arms(kind: i16, body_flags: u32) -> bool {
    match kind {
        1 => body_flags & flags::KIND_1 != 0,
        2 => body_flags & flags::KIND_2 != 0,
        3 => body_flags & flags::KIND_3 != 0,
        _ => true,
    }
}

/// The age and arming timers `f8f59..f9018`, once per step: the age
/// timer and step counter always advance; the arming accumulator runs
/// when the kind arms it (or flags 3 or 5 are set) and, once it reaches
/// one, raises a state below one to one.
pub fn advance_timers<M: Mem + MemMut>(m: &mut M, body: Body, def: Def) -> Result<(), Fault> {
    let age = body.age(m)? + body.age_step(m)?;
    let steps = body.steps(m)?.wrapping_add(1);
    body.set(m, Body::STEPS, steps)?;
    body.set(m, Body::AGE, age)?;
    let mut body_flags = body.flags(m)?;
    let armed = body_flags & flags::ARMING != 0;
    if !armed && body_flags & flags::ARMING_ON == 0 && !kind_arms(def.kind(m)?, body_flags) {
        return Ok(());
    }
    if !armed {
        body_flags |= flags::ARMING;
        body.set(m, Body::FLAGS, body_flags)?;
    }
    let arming = body.arming_rate(m)? + body.arming(m)?;
    body.set(m, Body::ARMING, arming)?;
    // `comiss` against one: below, or unordered, leaves it.
    if !(arming >= 1.0) {
        return Ok(());
    }
    if body.state(m)? < 1 {
        body.set(m, Body::STATE, 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::FlatRam;

    fn ram() -> (FlatRam, Body, Def) {
        (FlatRam::new(0x1000, 0x400), Body::at(0x1000), Def::at(0x1300))
    }

    #[test]
    fn mover_is_asked_unless_latched_and_not_skipped() {
        assert_eq!(mover_request(1, 0), None);
        assert_eq!(mover_request(1, flags::MOVER_SKIP), Some(false));
        assert_eq!(mover_request(0, flags::MOVER_FLAG), Some(true));
        assert_eq!(mover_request(0xfe, 0), Some(false));
    }

    #[test]
    fn update_adds_the_displacement_after_the_step() {
        let (mut m, b, _) = ram();
        b.set(&mut m, Body::VELOCITY, [1.0, 2.0, 3.0]).unwrap();
        let mut seen = None;
        update(
            &mut m,
            b,
            |_, flag| {
                assert!(!flag);
                Ok(Some([0.5, 0.25, -1.0]))
            },
            |m, v| {
                seen = Some(v);
                b.set(m, Body::VELOCITY, [10.0, 20.0, 30.0])
            },
        )
        .unwrap();
        assert_eq!(seen, Some([1.0, 2.0, 3.0]));
        assert_eq!(b.velocity(&m).unwrap(), [10.5, 20.25, 29.0]);
    }

    #[test]
    fn timers_arm_and_raise_the_state() {
        let (mut m, b, d) = ram();
        d.set(&mut m, Def::KIND, 1).unwrap();
        b.set(&mut m, Body::FLAGS, flags::KIND_1).unwrap();
        b.set(&mut m, Body::ARMING_RATE, 0.5).unwrap();
        b.set(&mut m, Body::AGE_STEP, 0.25).unwrap();
        advance_timers(&mut m, b, d).unwrap();
        assert_eq!(b.arming(&m).unwrap(), 0.5);
        assert_eq!(b.state(&m).unwrap(), 0);
        advance_timers(&mut m, b, d).unwrap();
        assert_eq!((b.state(&m).unwrap(), b.steps(&m).unwrap(), b.age(&m).unwrap()), (1, 2, 0.5));
        assert_ne!(b.flags(&m).unwrap() & flags::ARMING, 0);
    }

    #[test]
    fn unarmed_kinds_leave_the_accumulator() {
        let (mut m, b, d) = ram();
        d.set(&mut m, Def::KIND, 2).unwrap();
        b.set(&mut m, Body::ARMING_RATE, 2.0).unwrap();
        advance_timers(&mut m, b, d).unwrap();
        assert_eq!(b.arming(&m).unwrap(), 0.0);
        assert_eq!(b.steps(&m).unwrap(), 1);
    }
}
