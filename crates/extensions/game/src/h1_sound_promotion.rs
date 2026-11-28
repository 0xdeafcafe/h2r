//! Halo 1 impulse promotion (H1 0x24ea0 and 0x26e09). The native H2
//! promotion rules select a range within one definition (0x126ec0,
//! 0x218f10). H1 selects a different sound or rejects the request.

use guest_layout::GuestMemory;
use guest_layout::handle::DatumHandle;
use guest_layout::tags::GroupTag;
use guest_layout::view::Tags;
use hooks::{BuildError, Builder, Feature, Manifest, points};
use tags::sound::h1_promotion::*;

const SOUND: GroupTag = GroupTag::from_chars(*b"snd!");

pub struct H1SoundPromotion;

impl Feature for H1SoundPromotion {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-sound-promotion", "Halo 1 impulse promotion").doc("docs/halo1/sounds.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Each rule's credit and last time are in its tag, and the clock
        // is the mixer's: all guest memory. The sound table's start path
        // asks the promotion after the native admission checks
        // (`sim_bridge::sim_sound::promotion::promote_source`).
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        sim_bridge::sim_sound::promotion::set_source_promotion(Some(choose));
        Ok(())
    }
}

/// Returns the new credit and whether this request exceeded its allowance.
/// H1 adds one duration per request and subtracts elapsed mixer milliseconds.
fn charge(credit: u32, elapsed: u32, duration: u32, count: u32, has_target: bool) -> (u32, bool) {
    if count == 0 {
        return (credit, false);
    }
    let credit = credit.saturating_sub(elapsed);
    let next = credit.saturating_add(duration);
    if next <= count.saturating_mul(duration) {
        (next, false)
    } else {
        (if has_target { 0 } else { credit }, true)
    }
}

fn choose(m: &mut dyn GuestMemory, mut id: u32) -> Result<Option<u32>, guest_layout::MemFault> {
    let mut now = None;
    loop {
        let Some(tag) = Tags.entry(m, DatumHandle(id))? else {
            return Ok(Some(id));
        };
        if tag.group(m)? != SOUND || tag.size(m)? != SIZE {
            return Ok(Some(id));
        }
        let root = tag.address(m)?;
        if m.u32(root + SIGNATURE)? != MAGIC {
            return Ok(Some(id));
        }
        let count = m.u32(root + COUNT)?;
        if count == 0 {
            return Ok(Some(id));
        }
        let now = match now {
            Some(now) => now,
            None => {
                // H2 0x126a34 updates this clock in milliseconds.
                let time = m.u32(m.u32(points::SOUND_GLOBALS.base)? + 0x84)?;
                now = Some(time);
                time
            }
        };
        let target = m.u32(root + TARGET)?;
        let elapsed = now.wrapping_sub(m.u32(root + LAST_TIME)?);
        let (credit, exceeded) = charge(
            m.u32(root + CREDIT)?,
            elapsed,
            m.u32(root + MILLIS)?,
            count,
            target != u32::MAX,
        );
        m.set_u32(root + CREDIT, credit)?;
        m.set_u32(root + LAST_TIME, now)?;
        if !exceeded {
            return Ok(Some(id));
        }
        if target == u32::MAX {
            return Ok(None);
        }
        // Each exceeded rule with a target resets to zero. Even cyclic
        // source chains therefore reach an accepting rule on a revisit.
        id = target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_requests_promote_after_the_limit_and_idle_time_reopens_it() {
        let (credit, exceeded) = charge(0, 100, 500, 2, true);
        assert_eq!((credit, exceeded), (500, false));
        let (credit, exceeded) = charge(credit, 0, 500, 2, true);
        assert_eq!((credit, exceeded), (1000, false));
        assert_eq!(charge(credit, 0, 500, 2, true), (0, true));
        assert_eq!(charge(credit, 500, 500, 2, true), (1000, false));
        assert_eq!(charge(credit, 1000, 500, 2, true), (500, false));
    }

    #[test]
    fn a_promoted_impulse_uses_the_targets_own_rule() {
        use guest_layout::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, entry_layout, index_layout};

        let mut m = guest_layout::FlatMemory::new(0x0040_0000, 0x0020_0000);
        let index = 0x0059_0000;
        let entries = index + 0x100;
        let clock = index + 0x200;
        let roots = [index + 0x400, index + 0x500];
        let ids = [0xe000_0000, 0xe001_0001];
        m.set_u32(TAG_INDEX_GLOBAL, index).unwrap();
        m.set_u32(TAG_ENTRIES_GLOBAL, entries).unwrap();
        m.set_u32(index + index_layout::TAG_COUNT, 2).unwrap();
        m.set_u32(points::SOUND_GLOBALS.base, clock).unwrap();
        m.set_u32(clock + 0x84, 100).unwrap();
        for (i, root) in roots.into_iter().enumerate() {
            let entry = entries + i as u32 * entry_layout::STRIDE;
            m.set_u32(entry + entry_layout::GROUP, SOUND.0).unwrap();
            m.set_u32(entry + entry_layout::DATUM, ids[i]).unwrap();
            m.set_u32(entry + entry_layout::ADDRESS, root).unwrap();
            m.set_u32(entry + entry_layout::SIZE, SIZE).unwrap();
            m.set_u32(root + SIGNATURE, MAGIC).unwrap();
            m.set_u32(root + TARGET, ids[1 - i]).unwrap();
            m.set_u32(root + COUNT, 1).unwrap();
            m.set_u32(root + MILLIS, 250).unwrap();
        }
        assert_eq!(choose(&mut m, ids[0]).unwrap(), Some(ids[0]));
        assert_eq!(choose(&mut m, ids[0]).unwrap(), Some(ids[1]));
        assert_eq!(choose(&mut m, ids[1]).unwrap(), Some(ids[0]));
        m.set_u32(clock + 0x84, 350).unwrap();
        assert_eq!(choose(&mut m, ids[0]).unwrap(), Some(ids[0]));
    }

    #[test]
    fn a_rule_without_a_target_blocks_without_resetting() {
        assert_eq!(charge(500, 100, 500, 1, false), (400, true));
        assert_eq!(charge(400, 399, 500, 1, false), (1, true));
        assert_eq!(charge(1, 1, 500, 1, false), (500, false));
        assert_eq!(charge(0, 0, 500, 0, false), (0, false));
    }
}
