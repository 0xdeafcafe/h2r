//! Effect part dispatch, independent of guest memory.
//!
//! Retail's part creation switch (`0x17a8a0`, docs/engine/engine-map.md)
//! dispatches on a part's tag group. This crate names each group's class
//! and splits the two kinds of result:
//!
//! - sim state: a damage part (`jpt!`) changes health and spawns through
//!   the damage path; the order the parts are applied in is folded into
//!   [`EffectsSim::hash`];
//! - visual only: decals, lights and lens flares are queued as
//!   [`VisualEvent`]s for the renderer. They never reach the hash and the
//!   queue is not saved.
//!
//! The crate makes no guest call: [`EffectsLedger::guest_calls`] stays 0
//! (`effects_phase_ledger_is_zero`).

#![forbid(unsafe_code)]

use std::collections::VecDeque;

/// Tag group words as the part switch compares them (`0x17a8bd` and on).
pub mod group {
    pub const LENS: u32 = 0x6C65_6E73; // lens
    pub const MGS2: u32 = 0x4D47_5332; // MGS2
    pub const DECAL: u32 = 0x6465_6361; // deca
    pub const LIGHT: u32 = 0x6C69_6768; // ligh
    pub const DAMAGE: u32 = 0x6A70_7421; // jpt!
    pub const OBJECT: u32 = 0x6F62_6A65; // obje
    pub const SOUND: u32 = 0x736E_6421; // snd!
    pub const CHARACTER: u32 = 0x6368_6172; // char
    pub const COLN: u32 = 0x636F_6C6E; // coln: draws from the effects stream, nothing else
}

/// What a part does, by its group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartClass {
    /// `jpt!`: applies a damage effect at the part's origin.
    Damage,
    /// `obje`: creates an object.
    Object,
    /// `snd!`: starts a sound (the sound simulation's).
    Sound,
    /// `deca`: places a decal.
    Decal,
    /// `ligh`: creates a light.
    Light,
    /// `lens` and `MGS2`: retail returns without creating anything.
    Nothing,
    /// Any other group (`char`, `coln`, unknown): the retail switch's
    /// remaining arms, untouched.
    Other,
}

impl PartClass {
    pub fn of(group: u32) -> PartClass {
        match group {
            group::DAMAGE => PartClass::Damage,
            group::OBJECT => PartClass::Object,
            group::SOUND => PartClass::Sound,
            group::DECAL => PartClass::Decal,
            group::LIGHT => PartClass::Light,
            group::LENS | group::MGS2 => PartClass::Nothing,
            _ => PartClass::Other,
        }
    }

    /// The renderer's alone: nothing in the sim reads what it makes.
    pub fn visual_only(self) -> bool {
        matches!(self, PartClass::Decal | PartClass::Light | PartClass::Nothing)
    }
}

/// One part dispatch, as the bridge decodes it from the part record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub tick: u32,
    /// The effect datum's handle word (`[effect + 4]` is its tag).
    pub effect: u32,
    pub group: u32,
    /// The part's tag reference datum.
    pub tag: u32,
    pub location: i16,
    pub flags: u16,
}

/// What the caller does with the part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Run retail's creation: the part changes state the sim hash covers,
    /// or its arm is not proven free of random draws and sim writes.
    RunOriginal,
    /// Retail creates nothing for this part: return without the call.
    Skip,
}

/// A visual-only part for the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualEvent {
    /// Position in dispatch order, from 1; never reused.
    pub seq: u64,
    pub part: Part,
    pub class: PartClass,
    /// The guest also created the part (decals and lights, until the
    /// renderer reads these events instead of the guest arrays).
    pub guest_made: bool,
}

/// Calls and runs of the effects part split.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EffectsLedger {
    /// Guest calls the split made: it holds no guest-call capability.
    pub guest_calls: u64,
    pub parts: u64,
    pub damage_parts: u64,
    pub visual_events: u64,
    /// Parts retail would have created nothing for, returned without the call.
    pub skipped: u64,
    /// Visual events the queue dropped, oldest first, past its cap.
    pub dropped: u64,
}

/// The renderer's queue holds this many events.
pub const VISUAL_CAP: usize = 4096;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fold(mut h: u64, words: &[u32]) -> u64 {
    for w in words {
        for b in w.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    h
}

/// The part split: the damage chain the sim hash covers, and the visual queue.
#[derive(Clone, Debug)]
pub struct EffectsSim {
    damage_hash: u64,
    damage_count: u64,
    visual: VecDeque<VisualEvent>,
    next_seq: u64,
    ledger: EffectsLedger,
}

impl Default for EffectsSim {
    fn default() -> Self {
        EffectsSim {
            damage_hash: FNV_OFFSET,
            damage_count: 0,
            visual: VecDeque::new(),
            next_seq: 1,
            ledger: EffectsLedger::default(),
        }
    }
}

impl EffectsSim {
    pub fn new() -> Self {
        Self::default()
    }

    /// A part is about to be created. Damage parts extend the hashed chain;
    /// visual-only parts queue an event. Returns what to do with the part.
    pub fn part(&mut self, p: Part) -> Disposition {
        self.ledger.parts += 1;
        let class = PartClass::of(p.group);
        if class == PartClass::Damage {
            self.ledger.damage_parts += 1;
            self.damage_count += 1;
            self.damage_hash = fold(
                self.damage_hash,
                &[p.tick, p.effect, p.group, p.tag, u32::from(p.location as u16), u32::from(p.flags)],
            );
        }
        if !class.visual_only() {
            return Disposition::RunOriginal;
        }
        let skip = class == PartClass::Nothing;
        self.ledger.visual_events += 1;
        if skip {
            self.ledger.skipped += 1;
        }
        if self.visual.len() == VISUAL_CAP {
            self.visual.pop_front();
            self.ledger.dropped += 1;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.visual.push_back(VisualEvent { seq, part: p, class, guest_made: !skip });
        if skip { Disposition::Skip } else { Disposition::RunOriginal }
    }

    /// The renderer takes the events since its last call, in order.
    pub fn drain_visual(&mut self) -> Vec<VisualEvent> {
        self.visual.drain(..).collect()
    }

    pub fn pending_visual(&self) -> usize {
        self.visual.len()
    }

    /// The effects state the sim hash covers: the ordered damage parts.
    /// Visual events are not in it.
    pub fn hash(&self) -> u64 {
        fold(self.damage_hash, &[self.damage_count as u32, (self.damage_count >> 32) as u32])
    }

    pub fn ledger(&self) -> EffectsLedger {
        self.ledger
    }

    /// The checkpoint bytes: the damage chain only.
    pub fn save(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(16);
        v.extend_from_slice(&self.damage_hash.to_le_bytes());
        v.extend_from_slice(&self.damage_count.to_le_bytes());
        v
    }

    pub fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        let b: [u8; 16] = bytes.try_into().map_err(|_| format!("effects parts: {} bytes, want 16", bytes.len()))?;
        self.damage_hash = u64::from_le_bytes(b[..8].try_into().unwrap());
        self.damage_count = u64::from_le_bytes(b[8..].try_into().unwrap());
        self.visual.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(tick: u32, group: u32, tag: u32) -> Part {
        Part { tick, effect: 0x1234_0001 + tick, group, tag, location: 2, flags: 0x14 }
    }

    /// The retail switch's groups and what each arm does, from the
    /// disassembly of `0x17a8a0`: `lens` and `MGS2` jump to the epilogue
    /// (`0x17adc3`), every other named arm creates something.
    #[test]
    fn damage_parts_match_lifted() {
        let table = [
            (group::LENS, PartClass::Nothing),
            (group::MGS2, PartClass::Nothing),
            (group::DECAL, PartClass::Decal),
            (group::LIGHT, PartClass::Light),
            (group::DAMAGE, PartClass::Damage),
            (group::OBJECT, PartClass::Object),
            (group::SOUND, PartClass::Sound),
            (group::CHARACTER, PartClass::Other),
            (group::COLN, PartClass::Other),
            (0x7464_746C, PartClass::Other),
        ];
        for (g, class) in table {
            assert_eq!(PartClass::of(g), class, "{g:#x}");
        }
        assert_eq!(&group::DAMAGE.to_be_bytes(), b"jpt!");
        assert_eq!(&group::MGS2.to_be_bytes(), b"MGS2");

        // Every damage part runs retail's path and extends the chain, in
        // order: the same parts in another order hash differently, and a
        // chain re-folded from the inputs equals the sim's.
        let parts: Vec<_> = (0..40).map(|t| part(t, if t % 3 == 0 { group::DAMAGE } else { group::OBJECT }, 100 + t)).collect();
        let mut sim = EffectsSim::new();
        for p in &parts {
            assert_eq!(sim.part(*p), Disposition::RunOriginal);
        }
        let mut h = FNV_OFFSET;
        let mut n = 0u64;
        for p in parts.iter().filter(|p| p.group == group::DAMAGE) {
            h = fold(h, &[p.tick, p.effect, p.group, p.tag, u32::from(p.location as u16), u32::from(p.flags)]);
            n += 1;
        }
        assert_eq!(sim.hash(), fold(h, &[n as u32, (n >> 32) as u32]));
        assert_eq!(sim.ledger().damage_parts, n);
        let mut swapped = EffectsSim::new();
        let mut rev = parts.clone();
        rev.reverse();
        for p in rev {
            swapped.part(p);
        }
        assert_ne!(sim.hash(), swapped.hash());
    }

    #[test]
    fn visual_events_leave_hash_unchanged() {
        let damage: Vec<_> = (0..30).map(|t| part(t, group::DAMAGE, 7 + t)).collect();
        let mut plain = EffectsSim::new();
        let mut busy = EffectsSim::new();
        for (i, p) in damage.iter().enumerate() {
            plain.part(*p);
            busy.part(*p);
            for g in [group::DECAL, group::LIGHT, group::LENS, group::MGS2] {
                busy.part(part(i as u32, g, 9));
            }
            assert_eq!(plain.hash(), busy.hash(), "tick {i}");
        }
        assert!(busy.pending_visual() > 0);
        // Draining, or never draining past the cap, does not move it either.
        let h = busy.hash();
        let events = busy.drain_visual();
        assert_eq!(busy.hash(), h);
        assert!(events.windows(2).all(|w| w[0].seq < w[1].seq));
        for t in 0..(VISUAL_CAP as u32 * 2) {
            busy.part(part(t, group::DECAL, 1));
        }
        assert_eq!(busy.hash(), h);
        assert_eq!(busy.pending_visual(), VISUAL_CAP);
        assert_eq!(busy.ledger().dropped, VISUAL_CAP as u64);
        // Decals and lights are made by the guest too; lens and MGS2 are not.
        let one = EffectsSim::new().tap(|s| {
            s.part(part(0, group::LENS, 1));
            s.part(part(0, group::LIGHT, 1));
        });
        let ev: Vec<_> = one.visual.iter().map(|e| (e.class, e.guest_made)).collect();
        assert_eq!(ev, [(PartClass::Nothing, false), (PartClass::Light, true)]);
    }

    trait Tap: Sized {
        fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }
    impl Tap for EffectsSim {}

    #[test]
    fn effects_phase_ledger_is_zero() {
        let mut sim = EffectsSim::new();
        let mut skipped = 0;
        for t in 0..500u32 {
            let g = [group::DAMAGE, group::DECAL, group::LENS, group::OBJECT, group::LIGHT][(t % 5) as usize];
            if sim.part(part(t, g, t)) == Disposition::Skip {
                skipped += 1;
            }
        }
        let l = sim.ledger();
        assert_eq!(l.guest_calls, 0);
        assert_eq!(l.parts, 500);
        assert_eq!(l.damage_parts, 100);
        assert_eq!(l.visual_events, 300);
        assert_eq!(l.skipped, 100);
        assert_eq!(skipped, 100);
    }

    #[test]
    fn saved_chain_restores_into_a_fresh_sim() {
        let mut a = EffectsSim::new();
        a.part(part(1, group::DAMAGE, 5));
        a.part(part(2, group::DECAL, 5));
        let mut b = EffectsSim::new();
        b.load(&a.save()).unwrap();
        assert_eq!(a.hash(), b.hash());
        assert_eq!(b.pending_visual(), 0);
        a.part(part(3, group::DAMAGE, 6));
        b.part(part(3, group::DAMAGE, 6));
        assert_eq!(a.hash(), b.hash());
        assert!(b.load(&[0; 3]).is_err());
    }
}
