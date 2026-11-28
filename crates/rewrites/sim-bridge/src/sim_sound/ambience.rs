//! `0x126660`: the two background ambience slots blended toward their
//! candidates over guest memory (docs/engine/sound-instances.md, "Entries").
//!
//! | Data | Where |
//! |---|---|
//! | candidates | `eax`, two of `0x1c` bytes: fade time `+0`, id `+8`, two levels `+0xc` and `+0x10`, two more `+0x14` and `+0x18` |
//! | slots | sound globals `+0x1a8`, two of `0x1c` bytes: the candidate's layout, its weight at `+4` |
//! | the frame's time | sound globals `+0x1e0` |
//!
//! Each candidate with an id takes the free slot that holds it, or else
//! the last free slot whose weight is at most the floor. A slot moves toward
//! its candidate by at most the frame's time over the fade time a frame
//! (the first pair scaled by `[0x45dddc]`); a slot of weight at the floor
//! takes the first pair at once. A slot no candidate took fades out. The
//! comparisons are `comiss`: a NaN takes the branch an unordered compare
//! takes.

use engine::memory::{GuestMemory, MemFault};
use engine::points::SOUND_GLOBALS;

const NONE: u32 = u32::MAX;
/// `[0x45dc70]`: the weight floor and the least fade time.
pub const FLOOR: u32 = 0x0045_dc70;
/// `[0x45dddc]`: the first pair's rate scale.
pub const PAIR_SCALE: u32 = 0x0045_dddc;
/// `[0x45dbc0]`: a full weight, 1.
pub const FULL: u32 = 0x0045_dbc0;
pub const SLOTS: u32 = 0x1a8;
pub const FRAME_TIME: u32 = 0x1e0;
const ROW: u32 = 0x1c;
const COUNT: u32 = 2;

/// The step from `d` toward 0 held within `low` and `high`, low first: a
/// NaN `d` passes through.
fn clamp(d: f32, low: f32, high: f32) -> f32 {
    if low > d {
        low
    } else if d > high {
        high
    } else {
        d
    }
}

/// `0x126660(eax = candidates)`.
pub fn blend(m: &mut dyn GuestMemory, candidates: u32) -> Result<(), MemFault> {
    let floor = m.f32(FLOOR)?;
    let scale = m.f32(PAIR_SCALE)?;
    let full = m.f32(FULL)?;
    let g = m.u32(SOUND_GLOBALS.base)?;
    let slot = |j: u32| g.wrapping_add(SLOTS).wrapping_add(ROW * j);
    // `x > y` is `comiss x, y` then `ja`; its negation is `jbe`.
    let rate_of = |m: &dyn GuestMemory, time: f32| -> Result<f32, MemFault> {
        let time = if time > floor { time } else { floor };
        Ok(m.f32(g.wrapping_add(FRAME_TIME))? / time)
    };
    let mut used = 0u32;
    for i in 0..COUNT {
        let c = candidates.wrapping_add(ROW * i);
        let id = m.u32(c.wrapping_add(8))?;
        if id == NONE {
            continue;
        }
        let mut chosen = None;
        for j in 0..COUNT {
            if used & (1 << j) != 0 {
                continue;
            }
            let s = slot(j);
            if m.u32(s.wrapping_add(8))? == id {
                chosen = Some(j);
                break;
            }
            // `jb`: a weight above the floor, or unordered, is not free.
            if !(floor >= m.f32(s.wrapping_add(4))?) {
                continue;
            }
            chosen = Some(j);
        }
        let Some(j) = chosen else { continue };
        let rate = rate_of(&*m, m.f32(c)?)?;
        let s = slot(j);
        let at = |o: u32| s.wrapping_add(o);
        let to = |o: u32| c.wrapping_add(o);
        if !(floor >= m.f32(at(4))?) {
            let step = rate * scale;
            let low = 0.0 - step;
            let v = clamp(m.f32(to(0xc))? - m.f32(at(0xc))?, low, step);
            let sum = v + m.f32(at(0xc))?;
            m.set_f32(at(0xc), sum)?;
            let v = clamp(m.f32(to(0x10))? - m.f32(at(0x10))?, low, step);
            let sum = m.f32(at(0x10))? + v;
            m.set_f32(at(0x10), sum)?;
        } else {
            for o in [0xc, 0x10] {
                let word = m.u32(to(o))?;
                m.set_u32(at(o), word)?;
            }
        }
        let low = 0.0 - rate;
        let v = clamp(m.f32(to(0x18))? - m.f32(at(0x18))?, low, rate);
        let sum = m.f32(at(0x18))? + v;
        m.set_f32(at(0x18), sum)?;
        let v = clamp(m.f32(to(0x14))? - m.f32(at(0x14))?, low, rate);
        let sum = v + m.f32(at(0x14))?;
        m.set_f32(at(0x14), sum)?;
        let v = clamp(full - m.f32(at(4))?, low, rate);
        let sum = m.f32(at(4))? + v;
        m.set_f32(at(4), sum)?;
        let time = m.u32(c)?;
        m.set_u32(s, time)?;
        m.set_u32(at(8), id)?;
        used |= 1 << j;
    }
    for j in 0..COUNT {
        if used & (1 << j) != 0 {
            continue;
        }
        let s = slot(j);
        let rate = rate_of(&*m, m.f32(s)?)?;
        let weight = s.wrapping_add(4);
        let v = clamp(0.0 - m.f32(weight)?, 0.0 - rate, rate);
        let sum = m.f32(weight)? + v;
        m.set_f32(weight, sum)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const GLOBALS: u32 = 0x0060_0000;
    const CANDIDATES: u32 = 0x0061_0000;

    /// A frame of 0.1 s; a floor of 0, a pair scale of 2.
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.put_f32(FLOOR, 0.0).put_f32(PAIR_SCALE, 2.0).put_f32(FULL, 1.0);
        g.put_u32(SOUND_GLOBALS.base, GLOBALS).map(GLOBALS, 0x1000).put_f32(GLOBALS + FRAME_TIME, 0.1);
        g.map(CANDIDATES, 0x100);
        for i in 0..COUNT {
            g.put_u32(CANDIDATES + ROW * i + 8, NONE).put_u32(GLOBALS + SLOTS + ROW * i + 8, NONE);
        }
        g
    }

    fn slot(g: &FakeGuest, j: u32, o: u32) -> f32 {
        g.f32(GLOBALS + SLOTS + ROW * j + o).unwrap()
    }

    /// Candidate `i`: `id`, fade time 1 s, every level `level`.
    fn candidate(g: &mut FakeGuest, i: u32, id: u32, level: f32) {
        let c = CANDIDATES + ROW * i;
        g.put_f32(c, 1.0).put_u32(c + 8, id);
        for o in [0xc, 0x10, 0x14, 0x18] {
            g.put_f32(c + o, level);
        }
    }

    #[test]
    fn a_new_candidate_takes_a_free_slot_its_first_pair_at_once() {
        let mut g = guest();
        candidate(&mut g, 0, 7, 0.5);
        blend(&mut g, CANDIDATES).unwrap();
        // The last free slot: slot 1.
        assert_eq!(g.u32(GLOBALS + SLOTS + ROW + 8), Ok(7));
        assert_eq!((slot(&g, 1, 0xc), slot(&g, 1, 0x10)), (0.5, 0.5), "taken at once");
        assert_eq!((slot(&g, 1, 0x14), slot(&g, 1, 0x18)), (0.1, 0.1), "a rate of 0.1 a frame");
        assert_eq!(slot(&g, 1, 4), 0.1, "the weight rises by the rate");
        assert_eq!(slot(&g, 1, 0), 1.0, "the fade time is kept");
        assert_eq!(slot(&g, 0, 4), 0.0, "an unused slot at 0 stays");
    }

    #[test]
    fn a_held_candidate_moves_its_slot_by_the_scaled_rate() {
        let mut g = guest();
        candidate(&mut g, 0, 7, 1.0);
        let s = GLOBALS + SLOTS;
        g.put_u32(s + 8, 7).put_f32(s + 4, 0.5).put_f32(s, 1.0);
        blend(&mut g, CANDIDATES).unwrap();
        assert_eq!(g.u32(s + 8), Ok(7), "the slot that holds it");
        assert_eq!((slot(&g, 0, 0xc), slot(&g, 0, 0x10)), (0.2, 0.2), "0.1 * 2 a frame");
        assert_eq!(slot(&g, 0, 4), 0.6);
    }

    #[test]
    fn a_slot_no_candidate_takes_fades_out() {
        let mut g = guest();
        let s = GLOBALS + SLOTS + ROW;
        g.put_u32(s + 8, 9).put_f32(s + 4, 0.25).put_f32(s, 2.0);
        blend(&mut g, CANDIDATES).unwrap();
        assert_eq!(slot(&g, 1, 4), 0.2, "0.1 / 2 s a frame");
        g.put_f32(s + 4, 0.01);
        blend(&mut g, CANDIDATES).unwrap();
        assert_eq!(slot(&g, 1, 4), 0.0, "held at 0");
    }

    #[test]
    fn a_slot_above_the_floor_is_not_free() {
        let mut g = guest();
        for j in 0..COUNT {
            g.put_f32(GLOBALS + SLOTS + ROW * j + 4, 0.5).put_f32(GLOBALS + SLOTS + ROW * j, 1.0);
        }
        candidate(&mut g, 0, 7, 1.0);
        blend(&mut g, CANDIDATES).unwrap();
        assert!((0..COUNT).all(|j| g.u32(GLOBALS + SLOTS + ROW * j + 8) == Ok(NONE)), "no slot for it");
        assert_eq!(slot(&g, 0, 4), 0.4, "both fade");
    }

    #[test]
    fn the_clamp_takes_the_low_bound_first_and_passes_nan() {
        assert_eq!(clamp(-5.0, -1.0, 1.0), -1.0);
        assert_eq!(clamp(5.0, -1.0, 1.0), 1.0);
        assert_eq!(clamp(0.5, -1.0, 1.0), 0.5);
        assert!(clamp(f32::NAN, -1.0, 1.0).is_nan());
        assert_eq!(clamp(0.0, 1.0, -1.0), 1.0, "a negative rate: the low bound wins");
    }
}
