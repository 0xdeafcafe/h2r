//! `0x126ec0`: a sound's promotion rule and whether its cooldown refuses
//! the start, over the promotion state in the gestalt
//! (docs/engine/sound-instances.md, "The start layer").
//!
//! | Data | Where |
//! |---|---|
//! | the tag's promotion | definition `+0xc`, `0xff` for none |
//! | promotions | gestalt `+0x4c`, `0x1c` bytes: rule count `+0`, rules `+4` (16 bytes: `+2` enabled, `+4` seconds, `+8` threshold, `+0xc` increment), counter count `+8`, counters `+0xc`, active rule `+0x10`, last clock `+0x14`, cooldown end `+0x18` |
//! | the sound clock | sound globals `+0x84`, in ticks |
//!
//! The counting is `sim_sound::scheduling`'s, as the frame's native body
//! (`native_sound_scheduling::promotion`) uses.

use super::playback::{GESTALT, definition};
use crate::memory::{GuestMemory, MemFault};
use crate::points::SOUND_GLOBALS;
use sim_sound::scheduling::{Admission, PromotionCredit, admission, decay_counter};

const NONE: u32 = u32::MAX;
/// `[0x45dccc]`: ticks a second, the cooldown's scale.
pub const TICKS_PER_SECOND: u32 = 0x0045_dccc;
const CLOCK: u32 = 0x84;

/// `0x126ec0`'s answer: the rule it writes out and its `eax`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The rule index, -1 for a tag with no promotion.
    pub rule: u32,
    /// The cooldown refuses the start.
    pub refused: bool,
}

/// `cvttss2si`: the integer indefinite for NaN and out of range.
fn truncate(v: f32) -> u32 {
    if v.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&v) { 0x8000_0000 } else { v as i32 as u32 }
}

/// `0x126ec0(ecx = sound tag, &rule) -> eax`.
pub fn select(m: &mut dyn GuestMemory, tag: u32) -> Result<Rule, MemFault> {
    let none = Rule { rule: NONE, refused: false };
    let index = m.u8(definition(&*m, tag)?.wrapping_add(0xc))?;
    if index == 0xff {
        return Ok(none);
    }
    let promotions = m.u32(m.u32(GESTALT)?.wrapping_add(0x4c))?;
    let p = promotions.wrapping_add((i32::from(index as i8) * 0x1c) as u32);
    if p == 0 {
        return Ok(none);
    }
    let at = |o: u32| p.wrapping_add(o);
    let clock_at = m.u32(SOUND_GLOBALS.base)?.wrapping_add(CLOCK);
    if m.i32(at(0x18))? <= m.i32(clock_at)? {
        m.set_u32(at(0x10), NONE)?;
        m.set_u32(at(0x18), 0)?;
    }
    let elapsed = m.u32(clock_at)?.wrapping_sub(m.u32(at(0x14))?) as i32;
    let mut i = 0i32;
    while i < m.i32(at(8))? {
        let counter = m.u32(at(0xc))?.wrapping_add((i as u32).wrapping_mul(4));
        let left = decay_counter(m.i32(counter)?, elapsed);
        m.set_u32(counter, left as u32)?;
        i += 1;
    }
    // The rule passes up while each one it leaves reaches its threshold;
    // the counters are read through `+0xc` again at each rule, as a rule
    // may point them anywhere.
    let mut rule = 0i32;
    if m.i32(p)? > 0 {
        loop {
            let counter = m.u32(at(0xc))?.wrapping_add((rule as u32).wrapping_mul(4));
            let row = m.u32(at(4))?.wrapping_add((rule as u32).wrapping_mul(16));
            let credit = PromotionCredit::accrue(m.i32(counter)?, m.i32(row.wrapping_add(0xc))?);
            m.set_u32(counter, credit.0 as u32)?;
            if m.u16(row.wrapping_add(2))? as i16 <= 0
                || !credit.reached(m.i32(row.wrapping_add(8))?)
                || rule >= m.i32(p)?.wrapping_sub(1)
            {
                break;
            }
            m.set_u32(counter, 0)?;
            rule += 1;
            if rule >= m.i32(p)? {
                break;
            }
        }
    }
    let active = m.i32(at(0x10))?;
    let action = admission(rule, active, m.i32(clock_at)?, m.i32(at(0x18))?);
    if action == Admission::AdmitAndArm {
        m.set_u32(at(0x10), rule as u32)?;
        let seconds = m.f32(m.u32(at(4))?.wrapping_add((rule as u32) << 4).wrapping_add(4))?;
        let ticks = seconds * m.f32(TICKS_PER_SECOND)? + m.i32(clock_at)? as f32;
        m.set_u32(at(0x18), truncate(ticks))?;
    }
    let clock = m.u32(clock_at)?;
    m.set_u32(at(0x14), clock)?;
    Ok(Rule { rule: rule as u32, refused: action == Admission::Suppress })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::points::TAG_ENTRIES;
    use crate::testing::FakeGuest;

    const ENTRIES: u32 = 0x0060_0000;
    const DEF: u32 = 0x0060_1000;
    const GESTALT_AT: u32 = 0x0060_2000;
    const PROMOTIONS: u32 = 0x0060_3000;
    const RULES: u32 = 0x0060_4000;
    const COUNTERS: u32 = 0x0060_5000;
    const GLOBALS: u32 = 0x0060_6000;
    const TAG: u32 = 0xe100_0001;
    /// Promotion 1: two rules, both enabled, thresholds 2 and 1.
    const P: u32 = PROMOTIONS + 0x1c;

    fn guest(clock: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.put_u32(TAG_ENTRIES.base, ENTRIES).put_u32(ENTRIES + 16 + 8, DEF).put_u8(DEF + 0xc, 1);
        g.put_u32(GESTALT, GESTALT_AT).put_u32(GESTALT_AT + 0x4c, PROMOTIONS);
        g.put_u32(P, 2).put_u32(P + 4, RULES).put_u32(P + 8, 2).put_u32(P + 0xc, COUNTERS);
        g.put_u32(P + 0x10, NONE).put_u32(P + 0x14, clock).put_u32(P + 0x18, 0);
        for (i, (threshold, seconds)) in [(2u32, 1.0f32), (1, 2.0)].into_iter().enumerate() {
            let r = RULES + 16 * i as u32;
            g.put_u16(r + 2, 1).put_f32(r + 4, seconds).put_u32(r + 8, threshold).put_u32(r + 0xc, 1);
        }
        g.map(COUNTERS, 8);
        g.put_u32(SOUND_GLOBALS.base, GLOBALS).put_u32(GLOBALS + CLOCK, clock);
        g.put_f32(TICKS_PER_SECOND, 30.0);
        g
    }

    #[test]
    fn a_tag_with_no_promotion_starts_with_rule_minus_one() {
        let mut g = guest(0);
        g.put_u8(DEF + 0xc, 0xff);
        assert_eq!(select(&mut g, TAG), Ok(Rule { rule: NONE, refused: false }));
    }

    #[test]
    fn the_first_start_arms_rule_0_and_its_cooldown() {
        let mut g = guest(100);
        assert_eq!(select(&mut g, TAG), Ok(Rule { rule: 0, refused: false }));
        assert_eq!(g.u32(P + 0x10), Ok(0));
        assert_eq!(g.u32(P + 0x18), Ok(130), "1 s at 30 ticks a second");
        assert_eq!(g.u32(P + 0x14), Ok(100));
        assert_eq!(g.u32(COUNTERS), Ok(1));
    }

    #[test]
    fn a_second_start_on_the_same_tick_promotes_and_rearms() {
        let mut g = guest(100);
        select(&mut g, TAG).unwrap();
        // Rule 0 reaches 2: its credit is spent and rule 1 is chosen,
        // above the active rule, so it arms over the cooldown.
        assert_eq!(select(&mut g, TAG), Ok(Rule { rule: 1, refused: false }));
        assert_eq!((g.u32(COUNTERS), g.u32(COUNTERS + 4)), (Ok(0), Ok(1)));
        assert_eq!(g.u32(P + 0x18), Ok(160), "2 s from tick 100");
    }

    #[test]
    fn a_start_later_in_the_cooldown_decays_and_is_refused() {
        let mut g = guest(100);
        select(&mut g, TAG).unwrap();
        g.put_u32(GLOBALS + CLOCK, 110);
        // Ten ticks take rule 0's credit back to 0; rule 0 is the active
        // rule and its cooldown runs to 130.
        assert_eq!(select(&mut g, TAG), Ok(Rule { rule: 0, refused: true }));
        assert_eq!((g.u32(P + 0x18), g.u32(P + 0x14)), (Ok(130), Ok(110)));
    }

    #[test]
    fn a_lower_rule_in_the_cooldown_is_refused() {
        let mut g = guest(100);
        g.put_u32(P + 0x10, 1).put_u32(P + 0x18, 200);
        assert_eq!(select(&mut g, TAG), Ok(Rule { rule: 0, refused: true }));
        assert_eq!(g.u32(P + 0x10), Ok(1), "the active rule stays");
    }

    #[test]
    fn counters_decay_by_the_clock_since_the_last_start() {
        let mut g = guest(100);
        g.put_u32(COUNTERS, 5).put_u32(COUNTERS + 4, 1).put_u32(P + 0x14, 97);
        select(&mut g, TAG).unwrap();
        // 5 - 3 + 1 reaches 2 and is spent; 1 - 3 floors at 0, + 1.
        assert_eq!((g.u32(COUNTERS), g.u32(COUNTERS + 4)), (Ok(0), Ok(1)));
    }

    #[test]
    fn truncation_is_cvttss2si() {
        assert_eq!(truncate(3.9), 3);
        assert_eq!(truncate(-3.9), (-3i32) as u32);
        assert_eq!(truncate(f32::NAN), 0x8000_0000);
        assert_eq!(truncate(3.0e9), 0x8000_0000);
    }
}
