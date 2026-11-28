//! The script index's engine object (docs/game/game-types-without-carriers.md,
//! "No retail engine object"): a host-owned object at
//! `ENGINE_TABLE[SCRIPT_INDEX]` (`0x55e4e8`) whose slots make none of a
//! retail engine's rules, so a script game's record can carry
//! `rules::SCRIPT_INDEX` and every retail slot call finds a body.
//!
//! An engine object is one dword, its vtable of [`SLOTS`] thiscall slots.
//! Ours is Slayer's vtable as the image holds it (copied from guest
//! memory, `SLAYER_VTABLE`), with Slayer's own rule bodies replaced by the
//! empty bodies the other engines use in the same slots ([`OVERRIDES`]).
//! Every other slot of Slayer's is a body another engine shares, or one
//! that keeps the engine-globals entity working:
//!
//! - `+0x00` answers 2, Slayer's index, for the readers that ask the
//!   object which engine it is;
//! - `+0x04`, the start, zeroes the Slayer globals (`E+0xfc`) that the
//!   shared entity bodies `+0xb0` to `+0xb8` read;
//! - `+0x9c`, the engine-globals entity type, answers 0, Slayer's, so a
//!   client builds the same entity whatever the index.
//!
//! The slot map and the reads are in
//! docs/game/notes/2026-11-24-game-type-engine-reads.md, "GE4".
//!
//! [`register`] puts the object in at each game start (`GAME_ACTIVATE`,
//! before the engine start copies the table entry at `0x1574c0`), on every
//! machine, and answers the one retail jump on `E+0xc14` with no bounds
//! check ([`points::game_types::SPAWN_TYPE_MASK`]) as Slayer's case.

use std::sync::Arc;

use guest_layout::GuestMemory;
use guest_layout::MemFault;
use guest_layout::points;
use guest_layout::points::game_types::{SPAWN_TYPE_MASK, SPAWN_TYPE_MASK_SLAYER};
use runtime::TransferAbi;

/// Slots in every engine vtable.
pub const SLOTS: u32 = 51;
/// The object: its vtable pointer, then the vtable.
pub const OBJECT_LEN: u32 = 4 + 4 * SLOTS;
/// The engine index at `E+0xc14`.
pub const ENGINE_INDEX: u32 = 0xc14;

/// Slayer's own rule slots and the shared empty body each takes:
///
/// - `+0x14` (`0x2bbf70`, `ret 4`) resets the player's `+0x1b8`, which
///   only Slayer's and Juggernaut's bodies read; CTF, Oddball, KotH and
///   Territories have `0x72c70` (`ret 4`).
/// - `+0x18` (`0x2bbfa0`, `ret 4`) posts Slayer's start event (the start
///   line, GT5's); the empty `ret 4` is `0x72c70`.
/// - `+0x24` (`0x2bc110`, `ret 8`) walks the players for Slayer; every
///   other engine has `0x24dc50` (`ret 8`).
/// - `+0x78` (`0x2bc000`, `ret 0x10`) is the kill scoring
///   (`SLAYER_KILL`); Territories' `0x2ed7f0` is the empty `ret 0x10`.
pub const OVERRIDES: [(u32, u32); 4] = [
    (0x14, 0x0007_2c70),
    (0x18, 0x0007_2c70),
    (0x24, 0x0024_dc50),
    (0x78, 0x002e_d7f0),
];

/// The table entry the object goes in.
pub fn table_slot() -> u32 {
    points::ENGINE_TABLE.base + 4 * u32::from(::rules::SCRIPT_INDEX)
}

/// The object's vtable: Slayer's slots read from `m`, with [`OVERRIDES`].
/// Refuses a zero slot (the image is not mapped, or is another build).
pub fn vtable(m: &dyn GuestMemory) -> Result<Vec<u32>, Refusal> {
    let mut slots = Vec::with_capacity(SLOTS as usize);
    for i in 0..SLOTS {
        let at = points::SLAYER_VTABLE.base + 4 * i;
        let body = m.u32(at).map_err(Refusal::Fault)?;
        if body == 0 {
            return Err(Refusal::EmptySlot(4 * i));
        }
        slots.push(body);
    }
    for (offset, body) in OVERRIDES {
        slots[(offset / 4) as usize] = body;
    }
    Ok(slots)
}

/// Why the object was not put in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Fault(MemFault),
    /// Slayer's vtable has a zero at this offset.
    EmptySlot(u32),
    /// The table entry holds another object.
    Taken(u32),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Fault(e) => write!(f, "guest memory at {:#x} faulted", e.addr),
            Refusal::EmptySlot(o) => write!(f, "Slayer's vtable has no body at +{o:#x}"),
            Refusal::Taken(o) => write!(
                f,
                "the engine table's script index entry holds another object ({o:#010x})"
            ),
        }
    }
}

/// What [`install`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Installed {
    /// The entry already held the object.
    Kept,
    /// The vtable was written and the entry set.
    Written,
}

/// Puts the object at `object` (an [`OBJECT_LEN`]-byte guest block) in the
/// engine table's script index entry. An entry that already names it is
/// kept as it is; one that names another object is refused, since
/// nothing in the image writes that entry.
pub fn install(m: &mut dyn GuestMemory, object: u32) -> Result<Installed, Refusal> {
    let slot = table_slot();
    match m.u32(slot).map_err(Refusal::Fault)? {
        0 => {}
        o if o == object => return Ok(Installed::Kept),
        o => return Err(Refusal::Taken(o)),
    }
    let slots = vtable(&*m)?;
    let table = object + 4;
    let mut bytes = Vec::with_capacity(OBJECT_LEN as usize);
    bytes.extend_from_slice(&table.to_le_bytes());
    for s in &slots {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    m.write(object, &bytes).map_err(Refusal::Fault)?;
    m.write(slot, &object.to_le_bytes())
        .map_err(Refusal::Fault)?;
    Ok(Installed::Written)
}

/// Whether the spawn gather at [`SPAWN_TYPE_MASK`] is on the script index:
/// `esi` the engine globals.
pub fn on_script_index(m: &dyn GuestMemory, esi: u32) -> bool {
    m.u32(esi.wrapping_add(ENGINE_INDEX)).ok() == Some(u32::from(::rules::SCRIPT_INDEX))
}

/// The transfer to the spawn gather's Slayer case, if the image has it.
/// The case is a registered instruction entry (`boot::image_entries`),
/// which discovery makes a function entry of the image, not a resume: it
/// is entered by entry dispatch, and `has_resume` never holds for it.
pub fn slayer_case(d: &runtime::Dispatch) -> Option<TransferAbi> {
    d.has_generated(SPAWN_TYPE_MASK_SLAYER)
        .then(|| TransferAbi::jump(SPAWN_TYPE_MASK_SLAYER))
}

/// The hooks: the object at each game start, and the spawn gather's jump.
pub fn register(b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
    use hooks::Flow;
    let object = b.guest_alloc(OBJECT_LEN, ::runtime::guest::Permissions::RW)?;
    b.raw()
        .before(
            &points::GAME_ACTIVATE,
            "rules: the script index's engine object",
            move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                if let Err(e) = install(&mut *mem, object) {
                    eprintln!("rules: the script index has no engine object: {e}");
                }
                Ok(Flow::Continue)
            },
        )
        // After every other start hook (the skulls' is the last class's
        // other one): none reads the entry, and the engine start after
        // them all copies it.
        .last()
        .after_feature(crate::skulls_mp::FEATURE);
    b.raw()
        .before(
            &SPAWN_TYPE_MASK,
            "rules: the script index spawns by Slayer's mask bit",
            move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let index = {
                    let mem = rt.memory();
                    on_script_index(&*mem, c.cpu.esi)
                };
                if !index {
                    return Ok(Flow::Continue);
                }
                let Some(to) = rt.read_dispatch(slayer_case) else {
                    eprintln!(
                        "rules: the spawn gather's Slayer case {SPAWN_TYPE_MASK_SLAYER:#010x} is not an entry of this image"
                    );
                    return Ok(Flow::Continue);
                };
                // As the game's jump leaves it: ESI the index less one.
                c.cpu.esi = u32::from(::rules::SCRIPT_INDEX) - 1;
                Ok(Flow::Return(to))
            },
        )
        .optional();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const OBJECT: u32 = 0x2000_0000;

    /// A guest whose Slayer vtable holds invented bodies `0x1000 + slot`.
    fn guest() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(points::SLAYER_VTABLE.base, 4 * SLOTS)
            .map(points::ENGINE_TABLE.base, 4 * 11)
            .map(OBJECT, OBJECT_LEN);
        for i in 0..SLOTS {
            m.put_u32(points::SLAYER_VTABLE.base + 4 * i, 0x1000 + i);
        }
        m
    }

    #[test]
    fn the_object_is_slayers_vtable_with_its_rules_emptied() {
        let mut m = guest();
        assert_eq!(table_slot(), 0x0055_e4e8);
        assert_eq!(install(&mut m, OBJECT), Ok(Installed::Written));
        assert_eq!(m.u32(table_slot()), Ok(OBJECT));
        assert_eq!(m.u32(OBJECT), Ok(OBJECT + 4));
        for i in 0..SLOTS {
            let body = m.u32(OBJECT + 4 + 4 * i).unwrap();
            let want = OVERRIDES
                .iter()
                .find(|&&(o, _)| o == 4 * i)
                .map_or(0x1000 + i, |&(_, b)| b);
            assert_eq!(body, want, "+{:#x}", 4 * i);
        }
        // Slayer's kill slot is never the object's.
        assert_ne!(m.u32(OBJECT + 4 + 0x78), Ok(points::SLAYER_KILL.base));
        // The retail entries are untouched.
        for i in [0u32, 1, 2, 5, 7, 10] {
            assert_eq!(m.u32(points::ENGINE_TABLE.base + 4 * i), Ok(0), "{i}");
        }
        assert_eq!(install(&mut m, OBJECT), Ok(Installed::Kept));
    }

    #[test]
    fn a_taken_entry_or_an_empty_vtable_is_refused() {
        let mut m = guest();
        m.put_u32(table_slot(), 0x47_fc80);
        assert_eq!(install(&mut m, OBJECT), Err(Refusal::Taken(0x47_fc80)));
        assert_eq!(m.u32(OBJECT), Ok(0));
        let mut m = guest();
        m.put_u32(points::SLAYER_VTABLE.base + 0x30, 0);
        assert_eq!(install(&mut m, OBJECT), Err(Refusal::EmptySlot(0x30)));
        assert_eq!(m.u32(table_slot()), Ok(0));
    }

    #[test]
    fn the_spawn_gather_answers_only_the_script_index() {
        const E: u32 = 0x3000_0000;
        let mut m = FakeGuest::new();
        m.map(E, 0xc9c);
        for (index, ours) in [(2, false), (5, false), (6, true), (9, false)] {
            m.put_u32(E + ENGINE_INDEX, index);
            assert_eq!(on_script_index(&m, E), ours, "{index}");
        }
        assert!(!on_script_index(&m, 0x4000_0000), "an unmapped E");
    }

    /// The image makes the case a function entry, never a resume
    /// (`recomp::register_retail_entries`): the hook jumps to it by entry
    /// dispatch. Checking for a resume refused it on every image, and the
    /// game's unchecked jump then stopped on the script index's zero.
    #[test]
    fn the_spawn_gather_enters_slayers_case_as_the_image_builds_it() {
        assert_eq!(slayer_case(&runtime::Dispatch::default()), None);
        let mut d = runtime::Dispatch::default();
        d.register_generated_rust(SPAWN_TYPE_MASK_SLAYER, |_, _, _| unreachable!())
            .unwrap();
        assert!(!d.has_resume(SPAWN_TYPE_MASK_SLAYER));
        assert_eq!(
            slayer_case(&d),
            Some(TransferAbi::jump(SPAWN_TYPE_MASK_SLAYER))
        );
    }
}
