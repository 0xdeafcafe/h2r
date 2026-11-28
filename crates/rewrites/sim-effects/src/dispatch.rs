//! What the switch's Rust entry does with each part: every class is
//! Rust's, from the first part.
//!
//! - `jpt!`, `obje`, `snd!`, `char`, `coln`, `deca`, `ligh`: each its own
//!   answer, [`crate::damage`], [`crate::object`], [`crate::sound`],
//!   [`crate::character`], [`crate::coln`], [`crate::decal`],
//!   [`crate::light`]. Decal and light parts also queue a scene event.
//! - `lens`, `MGS2`, `tdtl`: retail makes nothing, so Rust returns with
//!   `EAX` = the group, as the epilogue does, and queues a flare event.
//! - every other group: retail's `0x17adb3` sets the warning byte
//!   [`WARNED`] once and returns ([`Action::Warn`]).
//!
//! A part Rust cannot answer (its inputs unreadable, or arithmetic that
//! could raise an unmasked exception) panics with its cause.

use crate::parts::{Part, PartClass};
use crate::scene::Queue;
use crate::{character, coln, damage, decal, light, object, sound};

/// The byte retail's arm for an unknown group sets (`0x17adb3`).
pub const WARNED: u32 = 0x0055_e756;

/// The decoded inputs of a part an arm handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inputs {
    Damage(damage::Inputs),
    Object(object::Inputs),
    Sound(sound::Inputs),
    Character(character::Inputs),
    Coln(coln::Inputs),
    Decal(decal::Inputs),
    Light(light::Inputs),
}

/// What Rust does with a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Return at once: retail's arm is the epilogue.
    Return,
    /// An unknown group: `AL` = [`WARNED`]'s byte, which is set to 1 if
    /// it was 0, then return.
    Warn,
    /// Rust calls the area damage with this record.
    Damage(damage::Call),
    /// Rust runs the object arm's leaves.
    Object(object::Inputs),
    /// Rust makes this sound call.
    Sound(sound::Call, sound::Inputs),
    /// Rust returns or makes the two calls.
    Character(character::Call, character::Inputs),
    /// Rust leaves the stream, the registers and the x87 state so.
    Coln(coln::Call),
    /// Rust runs the decal arm's leaves and draws.
    Decal(decal::Inputs),
    /// Rust makes this light call.
    Light(light::Call),
}

/// Parts by class, since the map load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub parts: u64,
    /// `lens`, `MGS2` and `tdtl`, returned from Rust.
    pub returned: u64,
    pub damage: u64,
    pub object: u64,
    pub sound: u64,
    pub character: u64,
    pub coln: u64,
    pub decal: u64,
    pub light: u64,
    /// Groups with no arm: the warning byte.
    pub other: u64,
}

impl Stats {
    pub const ZERO: Stats = Stats {
        parts: 0,
        returned: 0,
        damage: 0,
        object: 0,
        sound: 0,
        character: 0,
        coln: 0,
        decal: 0,
        light: 0,
        other: 0,
    };
}

/// The part switch's Rust state: the counters. Rust owned, rebuilt at
/// each map load.
#[derive(Clone, Debug, Default)]
pub struct Parts {
    stats: Stats,
}

impl Parts {
    pub const fn new() -> Self {
        Parts { stats: Stats::ZERO }
    }

    /// A part entered the switch. `inputs` is the arm's decoded input
    /// (`None` for the classes that read none). Visual parts go to
    /// `scene`.
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
            (PartClass::Other, _) => {
                s.other += 1;
                Action::Warn
            }
            (PartClass::Damage, Some(Inputs::Damage(i))) => {
                s.damage += 1;
                Action::Damage(damage::Call::predict(i))
            }
            (PartClass::Object, Some(Inputs::Object(i))) => {
                if i.mxcsr & object::MASKED != object::MASKED {
                    unimplemented!("effects parts: an object part under MXCSR {:#x}, an exception unmasked", i.mxcsr);
                }
                s.object += 1;
                Action::Object(*i)
            }
            (PartClass::Sound, Some(Inputs::Sound(i))) => {
                let Ok(call) = sound::Call::predict(i) else {
                    unimplemented!("effects parts: a sound part whose packing raises an unmasked exception");
                };
                s.sound += 1;
                Action::Sound(call, *i)
            }
            (PartClass::Character, Some(Inputs::Character(i))) => {
                s.character += 1;
                Action::Character(character::Call::predict(i), *i)
            }
            (PartClass::Coln, Some(Inputs::Coln(i))) => {
                let Ok(call) = coln::Call::predict(i) else {
                    unimplemented!("effects parts: a coln draw under an x87 state that faults");
                };
                s.coln += 1;
                Action::Coln(call)
            }
            (PartClass::Decal, Some(Inputs::Decal(i))) => {
                if !i.quiet_fpu() {
                    unimplemented!("effects parts: a decal part under an x87 state that could fault");
                }
                s.decal += 1;
                Action::Decal(*i)
            }
            (PartClass::Light, Some(Inputs::Light(i))) => {
                s.light += 1;
                Action::Light(light::Call::predict(i))
            }
            (class, _) => unimplemented!("effects parts: a {class:?} part without its inputs"),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// A new map.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Runs `ticks` damage parts with `visual` parts after each.
    fn run(ticks: u32, visual: &[u32]) -> (Vec<Action>, Parts, Queue) {
        let mut parts = Parts::new();
        let mut q = Queue::new();
        let mut damage = Vec::new();
        for t in 0..ticks {
            let p = part(t, group::DAMAGE);
            damage.push(parts.enter(&p, Some(&Inputs::Damage(damage_inputs(&p))), &mut q));
            for &g in visual {
                let a = parts.enter(&part(t, g), None, &mut q);
                assert_eq!(a, Action::Return, "{g:#x}");
            }
        }
        (damage, parts, q)
    }

    /// Visual parts touch no sim state: with them interleaved the damage
    /// parts get the same answers, in the same order, and the visual parts
    /// only queue scene events (Rust memory, not the arena).
    #[test]
    fn visual_events_leave_hash_unchanged() {
        let n = 300;
        let (plain, plain_parts, plain_q) = run(n, &[]);
        let visual = [group::LENS, group::MGS2];
        let (busy, busy_parts, busy_q) = run(n, &visual);
        assert_eq!(plain, busy);
        assert_eq!(plain_parts.stats().damage, busy_parts.stats().damage);
        assert!(busy.iter().all(|a| matches!(a, Action::Damage(_))));
        assert_eq!(plain_q.len(), 0);
        assert_eq!(busy_q.stats().queued, u64::from(n) * 2);
        assert_eq!(busy_parts.stats().returned, u64::from(n) * 2);
    }

    /// Every class is Rust's from its first part: no proof, no guest arm.
    #[test]
    fn every_arm_answers_its_first_part() {
        let mut parts = Parts::new();
        let mut q = Queue::new();
        let d = part(0, group::DAMAGE);
        let di = damage_inputs(&d);
        assert_eq!(parts.enter(&d, Some(&Inputs::Damage(di)), &mut q), Action::Damage(damage::Call::predict(&di)));
        let oi = crate::object::tests::inputs();
        assert_eq!(parts.enter(&part(0, group::OBJECT), Some(&Inputs::Object(oi)), &mut q), Action::Object(oi));
        let si = sound::Inputs { object: 7, tag: 9, scale: 1, ..sound::Inputs::default() };
        assert!(matches!(parts.enter(&part(0, group::SOUND), Some(&Inputs::Sound(si)), &mut q), Action::Sound(..)));
        let ci = character::Inputs { tag: 3, node: 2, ..character::Inputs::default() };
        assert!(matches!(parts.enter(&part(0, group::CHARACTER), Some(&Inputs::Character(ci)), &mut q), Action::Character(..)));
        let ni = coln::Inputs {
            entry: 0x0030_0000,
            seed_at: 0x0070_0004,
            seed: 1,
            lo: 0,
            hi: 1.0f32.to_bits(),
            scale: 0x3780_0000,
            fpu: x87::abi::X87Abi { control: 0x027f, ..x87::abi::X87Abi::default() },
        };
        assert!(matches!(parts.enter(&part(0, group::COLN), Some(&Inputs::Coln(ni)), &mut q), Action::Coln(_)));
        let di = crate::decal::tests::inputs();
        assert_eq!(parts.enter(&part(0, group::DECAL), Some(&Inputs::Decal(di)), &mut q), Action::Decal(di));
        let li = light::Inputs { object: u32::MAX, tag: 4, ..light::Inputs::default() };
        assert!(matches!(parts.enter(&part(0, group::LIGHT), Some(&Inputs::Light(li)), &mut q), Action::Light(_)));
        assert_eq!(parts.enter(&part(0, 0x6162_6364), None, &mut q), Action::Warn);
        let s = parts.stats();
        assert_eq!(
            [s.damage, s.object, s.sound, s.character, s.coln, s.decal, s.light, s.other, s.parts],
            [1, 1, 1, 1, 1, 1, 1, 1, 8]
        );
    }

    #[test]
    #[should_panic(expected = "effects parts: an object part under MXCSR 0x1f00")]
    fn an_unmasked_object_part_panics() {
        let unmasked = object::Inputs { mxcsr: 0x1f00, ..crate::object::tests::inputs() };
        Parts::new().enter(&part(0, group::OBJECT), Some(&Inputs::Object(unmasked)), &mut Queue::new());
    }

    #[test]
    #[should_panic(expected = "effects parts: a decal part under an x87 state that could fault")]
    fn a_loud_decal_part_panics() {
        let di = crate::decal::tests::inputs();
        let loud = decal::Inputs { fpu: x87::abi::X87Abi { control: 0x027e, ..di.fpu }, ..di };
        Parts::new().enter(&part(0, group::DECAL), Some(&Inputs::Decal(loud)), &mut Queue::new());
    }

    #[test]
    #[should_panic(expected = "effects parts: a Damage part without its inputs")]
    fn a_part_without_inputs_panics() {
        Parts::new().enter(&part(0, group::DAMAGE), None, &mut Queue::new());
    }
}
