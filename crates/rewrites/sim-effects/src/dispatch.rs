//! What the part switch's Rust entry does with each part.
//!
//! - `jpt!`: the damage arm ([`crate::arm`]) answers or audits.
//! - `lens`, `MGS2`, `tdtl`: retail makes nothing, so Rust returns with
//!   `EAX` = the group, as the epilogue does, and queues a flare event.
//! - `deca`, `ligh`: the guest's arm runs (it draws word +4 and writes the
//!   decal and light arrays the arena hash covers), and an event is queued.
//! - every other group: the guest's arm runs. Each is a named refusal.
//!
//! The arm's audit slot counts damage parts only, so the parts around them
//! never move which damage part is audited.

use crate::arm::{Arm, Decision};
use crate::damage::{self, Inputs, Record};
use crate::parts::{Part, PartClass};
use crate::scene::Queue;

/// Who answers a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The guest's arm runs.
    Guest,
    /// Return at once: retail's arm is the epilogue.
    Return,
    /// Rust calls the area damage with this record.
    Answer(Record),
}

/// Parts by what happened to them, since the map load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub parts: u64,
    /// `lens`, `MGS2` and `tdtl`, returned from Rust.
    pub returned: u64,
    /// `jpt!` parts, which the arm decided ([`crate::arm::Stats`]).
    pub damage: u64,
    /// Refusals: the guest's arm ran, by class.
    pub guest_object: u64,
    pub guest_sound: u64,
    pub guest_decal: u64,
    pub guest_light: u64,
    pub guest_character: u64,
    pub guest_coln: u64,
    pub guest_other: u64,
    /// Refusals: the part or its location could not be read.
    pub declined_fault: u64,
}

impl Stats {
    pub const ZERO: Stats = Stats {
        parts: 0,
        returned: 0,
        damage: 0,
        guest_object: 0,
        guest_sound: 0,
        guest_decal: 0,
        guest_light: 0,
        guest_character: 0,
        guest_coln: 0,
        guest_other: 0,
        declined_fault: 0,
    };

    /// Every part the guest's arm ran.
    pub fn refused(&self) -> u64 {
        self.guest_object
            + self.guest_sound
            + self.guest_decal
            + self.guest_light
            + self.guest_character
            + self.guest_coln
            + self.guest_other
            + self.declined_fault
    }
}

/// The part switch's Rust state: the damage arm and the counters. Rust
/// owned, rebuilt at each map load.
#[derive(Clone, Debug, Default)]
pub struct Parts {
    arm: Arm,
    stats: Stats,
}

impl Parts {
    pub const fn new() -> Self {
        Parts { arm: Arm::new(), stats: Stats::ZERO }
    }

    /// A part entered the switch. `damage` is the decoded record input
    /// (`None` when it could not be read). Visual parts go to `scene`.
    pub fn enter(&mut self, part: &Part, damage: Option<&Inputs>, scene: &mut Queue) -> Action {
        self.stats.parts += 1;
        let class = part.class();
        if class.visual() {
            scene.push(*part);
        }
        let s = &mut self.stats;
        match class {
            PartClass::Nothing => {
                s.returned += 1;
                return Action::Return;
            }
            PartClass::Damage => {
                let Some(inputs) = damage else {
                    s.declined_fault += 1;
                    self.arm.declined_fault();
                    return Action::Guest;
                };
                s.damage += 1;
                let record = damage::record(inputs);
                return match self.arm.decide(record) {
                    Decision::Answer => Action::Answer(record),
                    Decision::Audit => Action::Guest,
                };
            }
            PartClass::Object => s.guest_object += 1,
            PartClass::Sound => s.guest_sound += 1,
            PartClass::Decal => s.guest_decal += 1,
            PartClass::Light => s.guest_light += 1,
            PartClass::Character => s.guest_character += 1,
            PartClass::Coln => s.guest_coln += 1,
            PartClass::Other => s.guest_other += 1,
        }
        Action::Guest
    }

    /// A part whose record could not be read at all.
    pub fn fault(&mut self) {
        self.stats.parts += 1;
        self.stats.declined_fault += 1;
    }

    pub fn arm(&mut self) -> &mut Arm {
        &mut self.arm
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn arm_stats(&self) -> crate::arm::Stats {
        self.arm.stats()
    }

    /// A new map.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::{PROVEN_AFTER, Seen};
    use crate::parts::group;

    fn part(tick: u32, group: u32) -> Part {
        Part {
            tick,
            definition: 0xe000_0001,
            group,
            tag: 0xe100_0000 | tick,
            position: [tick as f32, 0.0, 1.0],
            direction: [0.0, 1.0, 0.0],
            scale: 1.0,
        }
    }

    fn inputs(p: &Part) -> Inputs {
        Inputs {
            tag: p.tag,
            owner: [p.tick, 2, 3],
            position: p.position.map(f32::to_bits),
            direction: p.direction.map(f32::to_bits),
            scale: p.scale.to_bits(),
            one: 1.0f32.to_bits(),
            ..Inputs::default()
        }
    }

    /// Runs `ticks` damage parts, each answered as the guest would (its
    /// record equal to the prediction), with `visual` parts after each.
    fn run(ticks: u32, visual: &[u32]) -> (Vec<Action>, Parts, Queue) {
        let mut parts = Parts::new();
        let mut q = Queue::new();
        let mut damage = Vec::new();
        for t in 0..ticks {
            let p = part(t, group::DAMAGE);
            let a = parts.enter(&p, Some(&inputs(&p)), &mut q);
            if a == Action::Guest {
                parts.arm().seen(&Seen { record: damage::record(&inputs(&p)), except: damage::NO_OBJECT });
            }
            damage.push(a);
            for &g in visual {
                let a = parts.enter(&part(t, g), None, &mut q);
                assert!(matches!(a, Action::Guest | Action::Return), "{g:#x}: {a:?}");
            }
        }
        (damage, parts, q)
    }

    /// Visual parts touch no sim state: with them interleaved the damage
    /// parts get the same answers and records, in the same order, and the
    /// visual parts only queue scene events (Rust memory, not the arena)
    /// or run the guest's own arm.
    #[test]
    fn visual_events_leave_hash_unchanged() {
        let n = PROVEN_AFTER as u32 + 300;
        let (plain, plain_parts, plain_q) = run(n, &[]);
        let visual = [group::DECAL, group::LIGHT, group::LENS, group::MGS2];
        let (busy, busy_parts, busy_q) = run(n, &visual);
        assert_eq!(plain, busy);
        assert_eq!(plain_parts.arm_stats(), busy_parts.arm_stats());
        assert!(busy.iter().any(|a| matches!(a, Action::Answer(_))));
        assert_eq!(plain_q.len(), 0);
        assert_eq!(busy_q.stats().queued, u64::from(n) * 4);
        let s = busy_parts.stats();
        assert_eq!((s.returned, s.guest_decal, s.guest_light), (u64::from(n) * 2, u64::from(n), u64::from(n)));
    }

    #[test]
    fn every_class_is_counted_by_name() {
        let mut parts = Parts::new();
        let mut q = Queue::new();
        for g in [group::OBJECT, group::SOUND, group::CHARACTER, group::COLN, 0x6162_6364] {
            assert_eq!(parts.enter(&part(0, g), None, &mut q), Action::Guest);
        }
        assert_eq!(parts.enter(&part(0, group::DAMAGE), None, &mut q), Action::Guest);
        parts.fault();
        let s = parts.stats();
        assert_eq!(
            [s.guest_object, s.guest_sound, s.guest_character, s.guest_coln, s.guest_other, s.declined_fault],
            [1, 1, 1, 1, 1, 2]
        );
        assert_eq!((s.parts, s.refused()), (7, 7));
        assert_eq!(parts.arm_stats().declined_fault, 1);
    }
}
