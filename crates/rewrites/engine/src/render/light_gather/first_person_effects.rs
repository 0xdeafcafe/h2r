//! The first-person effects: `0x00174220` with its argument 1, the call
//! `0x0003fdf0` makes after its prologue and before its record loop
//! (`0x0003fe43`; docs/graphics/native-objects.md, "First person").
//!
//! `0x00174220` walks the particle systems (`[0x00510c74]`, a data array
//! of `0x54`-byte systems) in index order and, for each one it selects,
//! calls the per-view particle update `0x00174420` with the system's
//! handle, the selection's two bytes and 0. That update stays the
//! guest's: its stamps are modelled only in R2's shadow arm
//! (`render_bridge::effects_shadow::view_update`, docs/graphics/retire-d3d.md),
//! and its particle draws are the guest's, so the port is the walk and the
//! selection, and each selected system's update is a guest call ([`run`]).
//!
//! The selection is read again after every update, as the guest's loop
//! reads the array again after each call: an update can change a later
//! system's fields.
//!
//! With argument 1, a system is drawn only when its effect's owner is the
//! current user and that user's camera is the first-person one
//! (`0x00155760` answers 0): the selection byte at `[esp+0x18]` must equal
//! the argument. The third-person half of the selection (`0x0017afd0`,
//! the owner's unit) decides nothing for argument 1 and reads only, so it
//! is not made.

use super::Result;
use super::Refusal;
use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::memory::GuestMemory;

/// `0x00174220`.
pub const ENTRY: u32 = 0x0017_4220;
/// The per-view particle update `0x00174420`: `(handle, first person,
/// flag, 0)`, `ret 0x10`.
pub const VIEW_UPDATE: u32 = 0x0017_4420;
/// A system's definition, `0x001751d0` (ECX the system).
pub const DEFINITION: u32 = 0x0017_51d0;
/// The particle systems' data array.
pub const SYSTEMS: u32 = 0x0051_0c74;
/// A system's size (`0x00174271`).
pub const SYSTEM: u32 = 0x54;
/// The effects' data array, `0x190`-byte effects, the owner at `+0x58`.
pub const EFFECTS: u32 = 0x004e_a93c;
const EFFECT: u32 = 0x190;
/// The tag table (`[TAGS] + 16 index`: group at `+0`, data at `+8`).
const TAGS: u32 = 0x004e_3b44;
/// The current user (`0x00174317`).
pub const USER: u32 = 0x004b_9ed8;
/// The users' cameras, `0x140` each from `+0`, the update at `+0xc`.
const CAMERAS: u32 = 0x004e_8c44;
/// The first-person camera update (`0x0023c110`) and the float it
/// compares its `+4` with.
const FIRST_PERSON_CAMERA: u32 = 0x0023_c110;
const FIRST_PERSON_ZERO: u32 = 0x0045_dbd8;
/// The users' records, `0x94` each, a word at `+0x42`.
const USERS: u32 = 0x004e_d284;
/// The clusters' bits a system's cluster at `+0x20` is tested in.
const CLUSTER_BITS: u32 = 0x0054_81a4;

/// The bytes below the cut's entry `ESP` that `0x0003fdf0`'s frame
/// (`0x48` and three registers, the argument and the return) and
/// `0x00174220`'s (`0x14` and four registers) keep: the update is called
/// at entry `ESP` - `0x80`, so its frame is where the guest's is.
pub const RESERVE: u32 = 0x80;
/// Where, below the cut's entry `ESP`, `0x00174220` keeps the two
/// selection bytes it pushes as dwords (`[esp+0x18]`, `[esp+0x1c]` at its
/// call): it writes only their low bytes, so the guest pushes what the
/// stack held above them.
const FIRST_PERSON_SLOT: u32 = 0x68;
const FLAG_SLOT: u32 = 0x64;

/// A system's definition (`0x001751d0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Definition {
    /// Its address.
    At(u32),
    /// A particle model's (`PRTM`, `prt3`): a virtual call on its parent's
    /// tag, so the guest's `0x001751d0` answers.
    Virtual,
}

/// `0x001751d0` for the system at `system`, where it reads only.
pub fn definition(m: &dyn GuestMemory, system: u32) -> Result<Definition> {
    let tags = m.u32(TAGS)?;
    let tag = m.u32(system + 0x10)?;
    let entry = tags.wrapping_add(((tag as u16 as i16 as i32) << 4) as u32);
    let group = m.u32(entry)?;
    let data = || -> Result<u32> { Ok(m.u32(tags.wrapping_add((tag & 0xffff) << 4).wrapping_add(8))?) };
    let event = u32::from(m.u16(system + 0xe)?) * 0x38;
    match group {
        // 'effe': the event's part list.
        0x6566_6665 => {
            let at = m.u32(data()?.wrapping_add(0x18))?;
            let part = m.u32(system + 0x14)?.wrapping_mul(0x38);
            Ok(Definition::At(m.u32(at.wrapping_add(part).wrapping_add(0x34))?.wrapping_add(event)))
        }
        0x6273_6474 => Ok(Definition::At(m.u32(data()?.wrapping_add(0x18))?.wrapping_add(event))),
        0x5052_544d | 0x7072_7433 => Ok(Definition::Virtual),
        // The guest answers 0 and then reads near it.
        _ => Err(Refusal::Unsupported),
    }
}

/// The first active system at or after `from` (`0x0016bc00` over the
/// array's bits), and its handle; `None` past the last.
pub fn next_active(m: &dyn GuestMemory, from: u32) -> Result<Option<(u32, u32)>> {
    let array = m.u32(SYSTEMS)?;
    let count = m.i32(array + 0x38)?;
    let bits = m.u32(array + 0x48)?;
    let mut i = from as i32;
    if i < 0 {
        return Ok(None);
    }
    while i < count {
        let word = m.u32(bits.wrapping_add(((i >> 5) as u32).wrapping_mul(4)))?;
        if word & (1 << (i & 31)) != 0 {
            let size = m.u32(array + 0x24)?;
            let data = m.u32(array + 0x44)?;
            let salt = m.u16(data.wrapping_add(size.wrapping_mul(i as u32)))? as i16 as i32;
            return Ok(Some((i as u32, ((salt << 16) as u32) | i as u32)));
        }
        i += 1;
    }
    Ok(None)
}

/// `0x0003eb20` (EDX `cluster`): whether the view's own cluster is
/// `cluster`.
fn view_cluster(m: &dyn GuestMemory, cluster: i32) -> Result<bool> {
    if cluster == -1 || m.u8(0x004b_9ee9)? == 0 {
        return Ok(false);
    }
    let index = m.u32(0x004b_9eec)?;
    if index == u32::MAX || m.u16(0x0046_86c4)? == 0xffff {
        return Ok(false);
    }
    let structure = m.u32(0x004e_0348)?;
    if m.i32(structure + 0xac)? <= 0 {
        return Ok(false);
    }
    let table = m.u32(structure + 0xb0)?;
    Ok(m.u16(table.wrapping_add(index.wrapping_mul(2)))? as i16 as i32 == cluster)
}

/// `0x00155760` (EAX `user`) answers 0: the user's camera is the
/// first-person one at rest.
fn first_person_camera(m: &dyn GuestMemory, user: u32) -> Result<bool> {
    let camera = CAMERAS.wrapping_add(user.wrapping_mul(0x140));
    Ok(m.u32(camera + 0xc)? == FIRST_PERSON_CAMERA && m.f32(camera + 4)? == m.f32(FIRST_PERSON_ZERO)?)
}

/// What `0x00174220` with argument 1 does for the system with `handle`
/// at `system` and definition `definition` (`0x00174260`..`0x001743be`):
/// the update's four arguments when it calls it, else `None`. `esp` is
/// the cut's entry `ESP`, where the selection bytes' upper bytes are.
pub fn select(m: &dyn GuestMemory, handle: u32, system: u32, definition: u32, esp: u32) -> Result<Option<[u32; 4]>> {
    let cluster = m.u16(system + 0x20)? as i16 as i32;
    if cluster == -1 {
        return Ok(None);
    }
    if (m.u16(definition + 0x16)? >> 9) & 1 == 0 {
        let word = m.u32(CLUSTER_BITS.wrapping_add(((cluster >> 5) as u32).wrapping_mul(4)))?;
        if word & (1 << (cluster & 31)) == 0 && !view_cluster(m, cluster)? {
            return Ok(None);
        }
    }
    if m.u32(system + 0x4c)? != u32::MAX || m.u32(system + 0x30)? == u32::MAX {
        return Ok(None);
    }
    let effect = m.u32(system + 0x18)?;
    let owner = if effect == u32::MAX {
        u32::MAX
    } else {
        let data = m.u32(m.u32(EFFECTS)? + 0x44)?;
        m.u32(data.wrapping_add((effect & 0xffff) * EFFECT).wrapping_add(0x58))?
    };
    let user = m.u32(USER)?;
    let first_person = user != u32::MAX && first_person_camera(m, user)?;
    // `[esp+0x18]`: the owner is the user, in first person. With
    // argument 1 only such a system is updated.
    if !(owner != u32::MAX && user == owner && first_person) {
        return Ok(None);
    }
    let mut drawn = true;
    if user != u32::MAX && m.u16(m.u32(USERS)?.wrapping_add(user.wrapping_mul(0x94)).wrapping_add(0x42))? != 0xffff {
        drawn = (m.u8(definition + 0x16)? >> 6) & 1 != 0;
    }
    if !drawn {
        return Ok(None);
    }
    let flag = u32::from(m.u16(definition + 0xc)? != 0);
    let high = |slot: u32| -> Result<u32> { Ok(m.u32(esp.wrapping_sub(slot))? & !0xff) };
    Ok(Some([handle, high(FIRST_PERSON_SLOT)? | 1, high(FLAG_SLOT)? | flag, 0]))
}

/// `0x00174220` with argument 1, from the cut of `0x0003fdf0` entered at
/// `esp`: each selected system's update is a guest call made where the
/// guest makes it. Answers the updates made, or why the walk stopped (a
/// read that faults, or a definition the guest reads near 0).
pub async fn run(cx: &Cx, esp: u32) -> std::result::Result<u32, Stop> {
    let mut from = 0;
    let mut made = 0;
    loop {
        let Some((index, handle)) = cx.read(|m| next_active(m, from))? else {
            return Ok(made);
        };
        let system = cx.read(|m| -> Result<u32> { Ok(m.u32(m.u32(SYSTEMS)? + 0x44)? + index * SYSTEM) })?;
        let definition = match cx.read(|m| definition(m, system))? {
            Definition::At(at) => at,
            Definition::Virtual => cx.call(GuestCall::new(DEFINITION).ecx(system).reserve(RESERVE)).await?.eax,
        };
        if definition == 0 {
            return Err(Stop::Refused(Refusal::Unsupported));
        }
        if let Some(args) = cx.read(|m| select(m, handle, system, definition, esp))? {
            cx.call(GuestCall::new(VIEW_UPDATE).args(args).reserve(RESERVE)).await?;
            made += 1;
        }
        from = index + 1;
    }
}

/// Why [`run`] stopped before the last system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    Refused(Refusal),
    Entry(EntryError),
}

impl From<Refusal> for Stop {
    fn from(r: Refusal) -> Self {
        Stop::Refused(r)
    }
}

impl From<EntryError> for Stop {
    fn from(e: EntryError) -> Self {
        Stop::Entry(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{self, EntryFuture, Regs, Ret};
    use guest_layout::testing::FakeGuest;

    const ARRAY: u32 = 0x0060_0000;
    const DATA: u32 = 0x0060_1000;
    const BITS: u32 = 0x0060_0800;
    const TAG_TABLE: u32 = 0x0061_0000;
    const TAG_DATA: u32 = 0x0062_0000;
    const DEFS: u32 = 0x0063_0000;
    const EFFECT_ARRAY: u32 = 0x0064_0000;
    const EFFECT_DATA: u32 = 0x0064_1000;
    const USER_TABLE: u32 = 0x0065_0000;
    const STACK: u32 = 0x0030_0000;

    /// Three systems of a 'bsdt' tag: 0 the user's in first person, 1
    /// not the user's, 2 the user's again; system 3 is inactive.
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(STACK - 0x1000, 0x2000)
            .map(0x0045_0000, 0x0010_0000)
            .map(ARRAY, 0x0010_0000);
        g.put_u32(SYSTEMS, ARRAY).put_u32(ARRAY + 0x38, 4).put_u32(ARRAY + 0x48, BITS).put_u32(ARRAY + 0x24, SYSTEM).put_u32(ARRAY + 0x44, DATA);
        g.put_u32(BITS, 0b0111);
        g.put_u32(TAGS, TAG_TABLE).put_u32(TAG_TABLE + 5 * 16, 0x6273_6474).put_u32(TAG_TABLE + 5 * 16 + 8, TAG_DATA);
        g.put_u32(TAG_DATA + 0x18, DEFS);
        // Definition 0: bit 9 set (no cluster test), drawn (bit 6), flag.
        g.put_u16(DEFS + 0x16, 1 << 9 | 1 << 6).put_u16(DEFS + 0xc, 1);
        g.put_u32(EFFECTS, EFFECT_ARRAY).put_u32(EFFECT_ARRAY + 0x44, EFFECT_DATA);
        g.put_u32(USER, 0).put_u32(CAMERAS + 0xc, FIRST_PERSON_CAMERA).put_f32(CAMERAS + 4, 0.0).put_f32(FIRST_PERSON_ZERO, 0.0);
        g.put_u32(USERS, USER_TABLE).put_u16(USER_TABLE + 0x42, 0xffff);
        for i in 0..4u32 {
            let s = DATA + i * SYSTEM;
            g.put_u16(s, 0x10 + i as u16).put_u32(s + 0x10, 5).put_u16(s + 0xe, 0).put_u16(s + 0x20, 0);
            g.put_u32(s + 0x4c, u32::MAX).put_u32(s + 0x30, 7).put_u32(s + 0x18, i);
            g.put_u32(EFFECT_DATA + i * EFFECT + 0x58, if i == 1 { 3 } else { 0 });
        }
        // The stack above the selection bytes.
        g.put_u32(STACK - FIRST_PERSON_SLOT, 0xaabb_cc00).put_u32(STACK - FLAG_SLOT, 0x1122_3300);
        g
    }

    fn body(cx: Cx) -> EntryFuture {
        Box::pin(async move {
            let esp = cx.entry().esp;
            let made = run(&cx, esp).await.map_err(|e| match e {
                Stop::Entry(e) => e,
                Stop::Refused(_) => EntryError::Stalled,
            })?;
            Ok(Ret::new(made, 4))
        })
    }

    #[test]
    fn the_users_first_person_systems_are_updated_in_index_order() {
        let regs = Regs { esp: STACK, ..Default::default() };
        let run = entry::testing::run(&body, guest(), regs, |_, _| Regs::default());
        assert_eq!(run.result, Ok(Ret::new(2, 4)));
        let calls: Vec<_> = run.calls.iter().map(|c| (c.target, c.args.clone(), c.reserve)).collect();
        assert_eq!(
            calls,
            [
                (VIEW_UPDATE, vec![0x0010_0000, 0xaabb_cc01, 0x1122_3301, 0], RESERVE),
                (VIEW_UPDATE, vec![0x0012_0002, 0xaabb_cc01, 0x1122_3301, 0], RESERVE),
            ]
        );
    }

    #[test]
    fn an_update_that_retires_a_later_system_is_seen() {
        let regs = Regs { esp: STACK, ..Default::default() };
        let run = entry::testing::run(&body, guest(), regs, |_, m| {
            // The first update retires system 2's location.
            m.set_u32(DATA + 2 * SYSTEM + 0x30, u32::MAX).unwrap();
            Regs::default()
        });
        assert_eq!(run.result, Ok(Ret::new(1, 4)));
    }

    #[test]
    fn a_third_person_camera_updates_nothing() {
        let mut g = guest();
        g.put_u32(CAMERAS + 0xc, 0x0023_cbb0);
        let run = entry::testing::run(&body, g, Regs { esp: STACK, ..Default::default() }, |_, _| Regs::default());
        assert_eq!(run.result, Ok(Ret::new(0, 4)));
        assert!(run.calls.is_empty());
    }

    #[test]
    fn a_particle_models_definition_is_the_guests() {
        let mut g = guest();
        g.put_u32(TAG_TABLE + 5 * 16, 0x5052_544d);
        let run = entry::testing::run(&body, g, Regs { esp: STACK, ..Default::default() }, |c, _| {
            Regs { eax: if c.target == DEFINITION { DEFS } else { 0 }, ..Default::default() }
        });
        assert_eq!(run.result, Ok(Ret::new(2, 4)));
        assert_eq!(run.calls.iter().filter(|c| c.target == DEFINITION).count(), 3);
        assert_eq!(run.calls[0].ecx, Some(DATA));
    }
}
