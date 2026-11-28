//! `GuestUnits`: the table's [`OwnerHost`] while units and script lines
//! live in guest memory (docs/engine/sound-instances.md, "Owners").
//!
//! `Sounds::step` calls the host synchronously; a unit's dialogue end
//! (`0x18c8c0`, ported as `engine::sim_sound::units::line_end`) and its
//! mouth (`0x18c6a0`, a guest call) run after the step from
//! [`GuestUnits::calls`], in the order the table asked for them. The script
//! line table's clear is a guest write, made at once and counted. They
//! move to the object store when it lands, with no change to `Sounds`.

use std::collections::BTreeSet;

use engine::memory::GuestMemory;
use sim_sound::impulse::Selection;
use sim_sound::instances::{Handle, Sounds, TagIndex};
use sim_sound::owners::{Owner, OwnerHost, Reason};

/// `[0x4ed288] + 0x128`: the 16 script line slots, each the handle of the
/// line's sound.
pub const LINES: u32 = 0x004e_d288;
pub const LINES_OFFSET: u32 = 0x128;
pub const LINE_SLOTS: u32 = 16;
const NONE: u32 = u32::MAX;

/// A unit call the phase makes after the step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnitCall {
    /// `0x18c8c0`: the unit's line `tag` ended.
    LineEnded { object: u32, tag: TagIndex },
    /// `0x18c6a0`: the unit's mouth for `tag`'s permutation, `ticks` in.
    Mouth { object: u32, tag: TagIndex, selection: Selection, ticks: u32 },
}

/// `sound.owner_guest_writes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerCounters {
    pub guest_writes: u64,
}

pub struct GuestUnits<'a> {
    m: &'a mut dyn GuestMemory,
    /// Objects the phase read as gone before the step.
    pub gone: BTreeSet<u32>,
    pub calls: Vec<UnitCall>,
    pub counters: OwnerCounters,
}

impl<'a> GuestUnits<'a> {
    pub fn new(m: &'a mut dyn GuestMemory) -> Self {
        Self { m, gone: BTreeSet::new(), calls: Vec::new(), counters: OwnerCounters::default() }
    }

    /// The guest memory the host writes, for reads beside the step.
    pub fn memory(&mut self) -> &mut dyn GuestMemory {
        self.m
    }

    /// Clear the script line slot that holds `handle`.
    fn clear_line(&mut self, handle: Handle) {
        let Ok(base) = self.m.u32(LINES) else { return };
        for slot in 0..LINE_SLOTS {
            let at = base.wrapping_add(LINES_OFFSET + 4 * slot);
            if matches!(self.m.u32(at), Ok(v) if v == handle.raw()) && self.m.write(at, &NONE.to_le_bytes()).is_ok() {
                self.counters.guest_writes += 1;
            }
        }
    }
}

impl OwnerHost for GuestUnits<'_> {
    fn alive(&mut self, owner: &Owner) -> bool {
        owner.unit().is_none_or(|object| !self.gone.contains(&object))
    }

    fn mouth(&mut self, owner: &Owner, tag: TagIndex, selection: Selection, ticks: u32) {
        if let Some(object) = owner.unit() {
            self.calls.push(UnitCall::Mouth { object, tag, selection, ticks });
        }
    }

    fn ended(&mut self, _: &Sounds, owner: &Owner, handle: Handle, tag: TagIndex, _: Reason) {
        if let Some(object) = owner.unit() {
            self.calls.push(UnitCall::LineEnded { object, tag });
        }
        if matches!(owner, Owner::ScriptLine(_)) {
            self.clear_line(handle);
        }
    }

    fn stopping(&mut self, _: &Owner, _: Handle, _: TagIndex) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;
    use sim_sound::instances::{Permutation, PitchRange, Request, Rng, SoundTag, TagTable};

    const TAG: TagIndex = TagIndex(0xe100_0001);
    const LINE_TABLE: u32 = 0x0060_0000;

    fn tags() -> TagTable {
        let mut t = TagTable::default();
        t.sounds.insert(TAG, SoundTag { range_count: 1, promotion: 0xff, ..SoundTag::default() });
        t.ranges.push(PitchRange { natural: [0, 0], playable: [0, 0], first_permutation: 0, count: 1 });
        t.permutations.push(Permutation { skip: 0, length_ms: Some(100) });
        t
    }

    fn admit(s: &mut Sounds, owner: Owner) -> Handle {
        let request = Request { owner, selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) };
        s.admit(&mut Rng::new(1), &tags(), request, 0).unwrap()
    }

    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(LINES & !0xfff, 0x1000).map(LINE_TABLE, 0x1000);
        g.put_u32(LINES, LINE_TABLE);
        for slot in 0..LINE_SLOTS {
            g.put_u32(LINE_TABLE + LINES_OFFSET + 4 * slot, NONE);
        }
        g
    }

    #[test]
    fn a_unit_line_end_calls_the_unit_once_with_the_tag() {
        let mut g = guest();
        let mut s = Sounds::new();
        admit(&mut s, Owner::Unit(0x55));
        let mut host = GuestUnits::new(&mut g);
        s.step(1, &mut host);
        s.step(3, &mut host);
        let ended: Vec<_> = host.calls.iter().filter(|c| matches!(c, UnitCall::LineEnded { .. })).collect();
        assert_eq!(ended, [&UnitCall::LineEnded { object: 0x55, tag: TAG }]);
        assert_eq!(host.calls[0], UnitCall::Mouth { object: 0x55, tag: TAG, selection: Selection { range: 0, permutation: 0, chunk: 0 }, ticks: 1 });
        assert_eq!(host.counters, OwnerCounters { guest_writes: 0 });
    }

    #[test]
    fn a_script_line_end_clears_its_slot() {
        let mut g = guest();
        let mut s = Sounds::new();
        let h = admit(&mut s, Owner::ScriptLine(None));
        g.put_u32(LINE_TABLE + LINES_OFFSET + 4 * 5, h.raw());
        let mut host = GuestUnits::new(&mut g);
        s.step(3, &mut host);
        assert!(host.calls.is_empty(), "a line with no unit calls no unit");
        assert_eq!(host.counters.guest_writes, 1);
        drop(host);
        assert_eq!(g.u32(LINE_TABLE + LINES_OFFSET + 4 * 5).unwrap(), NONE);
    }

    #[test]
    fn a_gone_unit_ends_its_line_with_reason_3() {
        let mut g = guest();
        let mut s = Sounds::new();
        admit(&mut s, Owner::Unit(9));
        let mut host = GuestUnits::new(&mut g);
        host.gone.insert(9);
        let events = s.step(1, &mut host);
        assert!(events.iter().any(|e| matches!(e, sim_sound::events::TableEvent::Ended { reason: Reason::OwnerGone, .. })));
    }
}
