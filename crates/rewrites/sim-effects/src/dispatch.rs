//! What the part switch's Rust entry does with each part.
//!
//! - `jpt!`, `obje`, `snd!`: each its own arm ([`crate::arm`]), which
//!   answers or audits: [`crate::damage`], [`crate::object`],
//!   [`crate::sound`].
//! - `lens`, `MGS2`, `tdtl`: retail makes nothing, so Rust returns with
//!   `EAX` = the group, as the epilogue does, and queues a flare event.
//! - `deca`, `ligh`: the guest's arm runs (it draws word +4 and writes the
//!   decal and light arrays the arena hash covers), and an event is queued.
//! - every other group: the guest's arm runs. Each is a named refusal.
//!
//! Each arm's audit slot counts its own parts only, so the parts around
//! them never move which part is audited.

use crate::arm::{self, Arm, Decision};
use crate::parts::{Part, PartClass};
use crate::scene::Queue;
use crate::{damage, object, sound};

/// The decoded inputs of a part an arm handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inputs {
    Damage(damage::Inputs),
    Object(object::Inputs),
    Sound(sound::Inputs),
}

/// Who answers a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The guest's arm runs.
    Guest,
    /// Return at once: retail's arm is the epilogue.
    Return,
    /// Rust calls the area damage with this record.
    Damage(damage::Call),
    /// Rust runs the object arm's leaves.
    Object(object::Inputs),
    /// Rust makes this sound call.
    Sound(sound::Call, sound::Inputs),
}

/// Parts by what happened to them, since the map load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub parts: u64,
    /// `lens`, `MGS2` and `tdtl`, returned from Rust.
    pub returned: u64,
    /// Parts each arm decided ([`arm::Stats`]).
    pub damage: u64,
    pub object: u64,
    pub sound: u64,
    /// Refusals: the guest's arm ran, by class.
    pub guest_decal: u64,
    pub guest_light: u64,
    pub guest_character: u64,
    pub guest_coln: u64,
    pub guest_other: u64,
    /// Refusals: the part or its inputs could not be read, or the arm's
    /// arithmetic could raise an unmasked exception (an object part under
    /// an MXCSR with an exception unmasked, a sound whose packing would).
    pub declined_fault: u64,
}

impl Stats {
    pub const ZERO: Stats = Stats {
        parts: 0,
        returned: 0,
        damage: 0,
        object: 0,
        sound: 0,
        guest_decal: 0,
        guest_light: 0,
        guest_character: 0,
        guest_coln: 0,
        guest_other: 0,
        declined_fault: 0,
    };

    /// Every part the guest's arm ran unchecked.
    pub fn refused(&self) -> u64 {
        self.guest_decal + self.guest_light + self.guest_character + self.guest_coln + self.guest_other + self.declined_fault
    }
}

/// The three arms' counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arms {
    pub damage: arm::Stats,
    pub object: arm::Stats,
    pub sound: arm::Stats,
}

/// The part switch's Rust state: the arms and the counters. Rust owned,
/// rebuilt at each map load.
#[derive(Clone, Debug, Default)]
pub struct Parts {
    pub damage: Arm<damage::Call>,
    pub object: Arm<object::Plan>,
    pub sound: Arm<sound::Call>,
    stats: Stats,
}

fn decide<T: arm::Prediction>(arm: &mut Arm<T>, prediction: T, answer: Action) -> Action {
    match arm.decide(prediction) {
        Decision::Answer => answer,
        Decision::Audit => Action::Guest,
    }
}

impl Parts {
    pub const fn new() -> Self {
        Parts { damage: Arm::new(), object: Arm::new(), sound: Arm::new(), stats: Stats::ZERO }
    }

    /// A part entered the switch. `inputs` is the arm's decoded input
    /// (`None` when it could not be read). Visual parts go to `scene`.
    pub fn enter(&mut self, part: &Part, inputs: Option<&Inputs>, scene: &mut Queue) -> Action {
        self.stats.parts += 1;
        let class = part.class();
        if class.visual() {
            scene.push(*part);
        }
        let s = &mut self.stats;
        match (class, inputs) {
            (PartClass::Nothing, _) => {
                s.returned += 1;
                Action::Return
            }
            (PartClass::Damage, Some(Inputs::Damage(i))) => {
                s.damage += 1;
                let call = damage::Call::predict(i);
                decide(&mut self.damage, call, Action::Damage(call))
            }
            (PartClass::Object, Some(Inputs::Object(i))) if i.mxcsr & object::MASKED == object::MASKED => {
                s.object += 1;
                decide(&mut self.object, object::Plan::new(*i), Action::Object(*i))
            }
            (PartClass::Sound, Some(Inputs::Sound(i))) => match sound::Call::predict(i) {
                Ok(call) => {
                    s.sound += 1;
                    decide(&mut self.sound, call, Action::Sound(call, *i))
                }
                Err(sound::Unmasked) => {
                    s.declined_fault += 1;
                    self.sound.declined_fault();
                    Action::Guest
                }
            },
            (PartClass::Damage | PartClass::Object | PartClass::Sound, _) => {
                s.declined_fault += 1;
                match class {
                    PartClass::Damage => self.damage.declined_fault(),
                    PartClass::Object => self.object.declined_fault(),
                    _ => self.sound.declined_fault(),
                }
                Action::Guest
            }
            (PartClass::Decal, _) => {
                s.guest_decal += 1;
                Action::Guest
            }
            (PartClass::Light, _) => {
                s.guest_light += 1;
                Action::Guest
            }
            (PartClass::Character, _) => {
                s.guest_character += 1;
                Action::Guest
            }
            (PartClass::Coln, _) => {
                s.guest_coln += 1;
                Action::Guest
            }
            (PartClass::Other, _) => {
                s.guest_other += 1;
                Action::Guest
            }
        }
    }

    /// A part whose record could not be read at all.
    pub fn fault(&mut self) {
        self.stats.parts += 1;
        self.stats.declined_fault += 1;
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn arms(&self) -> Arms {
        Arms { damage: self.damage.stats(), object: self.object.stats(), sound: self.sound.stats() }
    }

    /// A new map.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::PROVEN_AFTER;
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

    fn damage_inputs(p: &Part) -> damage::Inputs {
        damage::Inputs {
            tag: p.tag,
            owner: [p.tick, 2, 3],
            position: p.position.map(f32::to_bits),
            direction: p.direction.map(f32::to_bits),
            scale: p.scale.to_bits(),
            one: 1.0f32.to_bits(),
            ..damage::Inputs::default()
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
            let i = damage_inputs(&p);
            let a = parts.enter(&p, Some(&Inputs::Damage(i)), &mut q);
            if a == Action::Guest {
                parts.damage.seen(&damage::Call::predict(&i));
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
        assert_eq!(plain_parts.arms(), busy_parts.arms());
        assert!(busy.iter().any(|a| matches!(a, Action::Damage(_))));
        assert_eq!(plain_q.len(), 0);
        assert_eq!(busy_q.stats().queued, u64::from(n) * 4);
        let s = busy_parts.stats();
        assert_eq!((s.returned, s.guest_decal, s.guest_light), (u64::from(n) * 2, u64::from(n), u64::from(n)));
    }

    /// Object and sound parts have their own arms: each is proven by its
    /// own matches, its audit slots count only its parts, and a damage
    /// mismatch leaves them answering.
    #[test]
    fn object_and_sound_parts_answer_after_their_own_proof() {
        let mut parts = Parts::new();
        let mut q = Queue::new();
        let oi = crate::object::tests::inputs();
        let si = sound::Inputs { object: 7, tag: 9, scale: 1, ..sound::Inputs::default() };
        let (obj, snd) = (part(0, group::OBJECT), part(0, group::SOUND));
        for _ in 0..PROVEN_AFTER {
            assert_eq!(parts.enter(&obj, Some(&Inputs::Object(oi)), &mut q), Action::Guest);
            let plan = parts.object.pending_mut().copied().unwrap();
            parts.object.verdict(plan.place_difference(&object::PlaceSeen {
                words: oi.copies(),
                args: oi.place_args(),
                regs: oi.place_regs(),
            }));
            assert_eq!(parts.enter(&snd, Some(&Inputs::Sound(si)), &mut q), Action::Guest);
            parts.sound.seen(&sound::Call::predict(&si).unwrap());
        }
        // A damage part that differs does not stop the other arms.
        let d = part(1, group::DAMAGE);
        parts.enter(&d, Some(&Inputs::Damage(damage_inputs(&d))), &mut q);
        parts.damage.seen(&damage::Call { except: 5, ..damage::Call::predict(&damage_inputs(&d)) });
        // Slot 4096 of each is an audit, then they answer.
        assert_eq!(parts.enter(&obj, Some(&Inputs::Object(oi)), &mut q), Action::Guest);
        assert_eq!(parts.enter(&obj, Some(&Inputs::Object(oi)), &mut q), Action::Object(oi));
        assert_eq!(parts.enter(&snd, Some(&Inputs::Sound(si)), &mut q), Action::Guest);
        assert!(matches!(parts.enter(&snd, Some(&Inputs::Sound(si)), &mut q), Action::Sound(..)));
        let a = parts.arms();
        assert_eq!((a.object.answered, a.sound.answered, a.damage.mismatched), (1, 1, 1));
    }

    #[test]
    fn every_class_is_counted_by_name() {
        let mut parts = Parts::new();
        let mut q = Queue::new();
        for g in [group::CHARACTER, group::COLN, 0x6162_6364] {
            assert_eq!(parts.enter(&part(0, g), None, &mut q), Action::Guest);
        }
        for g in [group::DAMAGE, group::OBJECT, group::SOUND] {
            assert_eq!(parts.enter(&part(0, g), None, &mut q), Action::Guest);
        }
        // An object part under an MXCSR with an exception unmasked.
        let unmasked = object::Inputs { mxcsr: 0x1f00, ..crate::object::tests::inputs() };
        assert_eq!(parts.enter(&part(0, group::OBJECT), Some(&Inputs::Object(unmasked)), &mut q), Action::Guest);
        parts.fault();
        let s = parts.stats();
        assert_eq!([s.guest_character, s.guest_coln, s.guest_other, s.declined_fault], [1, 1, 1, 5]);
        assert_eq!((s.parts, s.refused()), (8, 8));
        let a = parts.arms();
        assert_eq!([a.damage.declined_fault, a.object.declined_fault, a.sound.declined_fault], [1, 2, 1]);
    }
}
