//! A speaking unit's mouth over guest memory: the owner update `0x18c6a0`
//! and its body `0x18c720`, the sound class `0x221810`, the extra-info
//! resource's residency (`0x12de70`, flags 2), the aperture sampler
//! `0x218e50`, the aperture step `0x18c9b0` and the facial sequence
//! `0x18ca20` (docs/engine/sound-instances.md, "Owners";
//! docs/halo1/sounds.md, "Dialogue mouth curves").
//!
//! [`gate`] runs everything up to the residency answer and [`finish`]
//! the rest, so a caller can ask the guest's resource cache in between
//! when [`residency`] says the answer needs its I/O ([`Residency::Guest`]).
//!
//! | Data | Where |
//! |---|---|
//! | a `snd!` definition | `+1` flags (bit 1: no lip sync), `+2` i8 class, `+8` i16 first pitch range, `+0xe` i16 extra info |
//! | the gestalt `[0x51ebd4]` | pitch ranges `+0x24` (12 bytes: `+8` i16 first permutation, `+0xa` i16 count), permutations `+0x2c` (16 bytes: `+5` i8 descriptor), extra infos `+0x54` (`0x2c` bytes: `+0` count, `+4` runtime `R`, `+8` the resource) |
//! | `R` | `+4` the data buffer, `+0xc` the descriptors (16 bytes: mouth offset, mouth length, facial offset, facial length) |
//! | sound classes | `[[0x4e034c] + 0xc4]` when `+0xc0` is set, its `+4` tag; that definition's `+4`, `0x5c` a class; `+8` bit 1 is lip sync |
//! | the unit's aperture | object `+0x208`, a step of at most 0.3 a tick |
//! | the resource cache | `[0x4e649c]` (slots `[+0x64]+0x44`, 24 bytes, stamp `+0x14` from `+0x38`), requests `[0x4e648c]+0x44` (12 bytes: `+2` done, `+3` priority), phase `[0x4e64a0]`, filter `[0x4e64a4]`, structure misses `[0x4e64a8]` |

use super::units::{DIALOGUE, DIALOGUE_ON, UNIT_TYPES, object_of_types};
use crate::memory::{GuestMemory, MemFault};
use crate::points::{GLOBALS_TAG, TAG_ENTRIES};
use crate::sim_anim::facial::{self, Curve, FCW, MXCSR};
use sim_core::det::{self, Fp, X87};
use std::cmp::Ordering;

const NONE: u32 = u32::MAX;
/// `0x18c6a0(owner, payload, tag, range, permutation, seconds)`, `ret 0x18`.
pub const UNIT_MOUTH: u32 = 0x0018_c6a0;
/// `0x12de70(ecx = resource, eax = flags) -> al`: the resource cache's
/// touch, which the guest keeps for its I/O.
pub const CACHE_TOUCH: u32 = 0x0012_de70;
/// The flags the mouth touches with: request, not blocking.
pub const TOUCH_FLAGS: u32 = 2;
/// `[0x51ebd4]`: the sound gestalt.
pub const GESTALT: u32 = 0x0051_ebd4;
/// `0x45dcfc`: ticks a second.
const TICKS: u32 = 0x41f0_0000;
/// `0x45dbb4`: an aperture byte's scale, 1/255.
const BYTE: u32 = 0x3b80_8081;
/// `0x45df30`, `0x44ae90`: the aperture step's bounds.
const STEP_DOWN: u32 = 0xbe99_999a;
const STEP_UP: u32 = 0x3e99_999a;
/// Object `+0x208`: the unit's aperture.
pub const APERTURE: u32 = 0x208;

const CACHE: u32 = 0x004e_649c;
const REQUESTS: u32 = 0x004e_648c;
const LOAD_PHASE: u32 = 0x004e_64a0;
const LOAD_FILTER: u32 = 0x004e_64a4;
const STRUCTURE_MISSES: u32 = 0x004e_64a8;

/// `0x18c6a0`'s arguments. The payload is not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub unit: u32,
    pub tag: u32,
    pub range: u32,
    /// The permutation record's address.
    pub permutation: u32,
    /// Seconds into the sound, f32 bits.
    pub seconds: u32,
}

impl Call {
    pub fn from_args(args: [u32; 6]) -> Self {
        let [unit, _payload, tag, range, permutation, seconds] = args;
        Call {
            unit,
            tag,
            range,
            permutation,
            seconds,
        }
    }
}

/// What [`residency`] answers for the extra-info resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residency {
    /// Loaded; its cache slot is stamped.
    Resident,
    /// Not loaded, and nothing to ask the cache for.
    Absent,
    /// The answer needs the cache's I/O: a request to submit
    /// (`0x12e3a0`), a priority to raise (`0x120ce0`), a blocking wait or
    /// a finished load to relocate (`0x223c70`). Nothing was written.
    Guest,
}

/// The mouth past its first tests: the permutation's index in its range
/// and the extra-info element whose resource the cache answers for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    pub index: u32,
    pub extra: u32,
    pub residency: Residency,
}

impl Pending {
    /// The resource the cache touches: the element's `+8`.
    pub fn resource(&self) -> u32 {
        self.extra.wrapping_add(8)
    }
}

fn definition(m: &dyn GuestMemory, tag: u32) -> Result<u32, MemFault> {
    let entries = m.u32(TAG_ENTRIES.base)?;
    m.u32(entries.wrapping_add((tag & 0xffff) << 4).wrapping_add(8))
}

fn i16_at(m: &dyn GuestMemory, at: u32) -> Result<i32, MemFault> {
    Ok(i32::from(m.u16(at)? as i16))
}

/// The pitch range row `range` of `def`.
fn range_row(m: &dyn GuestMemory, def: u32, range: i32) -> Result<u32, MemFault> {
    let gestalt = m.u32(GESTALT)?;
    let row = i16_at(m, def.wrapping_add(8))?.wrapping_add(range);
    Ok(m.u32(gestalt.wrapping_add(0x24))?
        .wrapping_add(row.wrapping_mul(12) as u32))
}

fn extra_info(m: &dyn GuestMemory, def: u32) -> Result<u32, MemFault> {
    let gestalt = m.u32(GESTALT)?;
    Ok(m.u32(gestalt.wrapping_add(0x54))?
        .wrapping_add(i16_at(m, def.wrapping_add(0xe))?.wrapping_mul(0x2c) as u32))
}

/// `0x221810(class)`: the sound class record.
pub fn sound_class(m: &dyn GuestMemory, class: i32) -> Result<u32, MemFault> {
    let globals = m.u32(GLOBALS_TAG.base)?;
    let list = if m.u32(globals.wrapping_add(0xc0))? != 0 {
        m.u32(globals.wrapping_add(0xc4))?
    } else {
        0
    };
    let classes = definition(m, m.u32(list.wrapping_add(4))?)?;
    Ok(m.u32(classes.wrapping_add(4))?
        .wrapping_add(class.wrapping_mul(0x5c) as u32))
}

/// The cache slot of request `handle` takes the cache's epoch.
fn stamp(m: &mut dyn GuestMemory, handle: u32) -> Result<(), MemFault> {
    let cache = m.u32(CACHE)?;
    let slots = m.u32(m.u32(cache.wrapping_add(0x64))?.wrapping_add(0x44))?;
    let epoch = m.u32(cache.wrapping_add(0x38))?;
    m.set_u32(
        slots
            .wrapping_add((handle & 0xffff) * 0x18)
            .wrapping_add(0x14),
        epoch,
    )
}

/// `0x12de70(ecx = resource, eax = 2)` where it needs no I/O.
pub fn residency(m: &mut dyn GuestMemory, resource: u32) -> Result<Residency, MemFault> {
    if m.u8(resource.wrapping_add(0x1e))? != 0 {
        let handle = m.u32(resource.wrapping_add(0x20))?;
        stamp(m, handle)?;
        return Ok(Residency::Resident);
    }
    if m.i32(resource.wrapping_add(8))? <= 0 || m.i32(resource.wrapping_add(0xc))? < 0 {
        return Ok(Residency::Absent);
    }
    let entries = m.u32(TAG_ENTRIES.base)?;
    let group =
        m.u32(entries.wrapping_add((i16_at(&*m, resource.wrapping_add(0x18))? << 4) as u32))?;
    let structure = group == u32::from_be_bytes(*b"ltmp") || group == u32::from_be_bytes(*b"sbsp");
    // Flags 2 asks at priority 2; with loading on (phase above 0), a wait
    // under the filter, and in phase 1 that wait blocks (`0x12df1e..`).
    let phase = m.i32(LOAD_PHASE)?;
    let filter = m.u32(LOAD_FILTER)?;
    let wait = phase > 0 && (filter == 0 || (filter == 1 && structure));
    let blocking = phase == 1 && wait;
    let handle = m.u32(resource.wrapping_add(0x20))?;
    if handle == NONE {
        return Ok(Residency::Guest);
    }
    let requests = m.u32(m.u32(REQUESTS)?.wrapping_add(0x44))?;
    let entry = requests.wrapping_add((handle & 0xffff) * 12);
    if m.u8(entry.wrapping_add(2))? != 0 || m.u8(entry.wrapping_add(3))? < 2 || blocking {
        return Ok(Residency::Guest);
    }
    stamp(m, handle)?;
    if structure {
        let misses = m.u32(STRUCTURE_MISSES)?;
        m.set_u32(STRUCTURE_MISSES, misses.wrapping_add(1))?;
    }
    Ok(Residency::Absent)
}

/// `0x18c6a0` and `0x18c720` up to the residency answer: `None` when the
/// mouth stops before it (no unit, no permutation, not a unit, a sound
/// without lip sync or without an extra info).
pub fn gate(m: &mut dyn GuestMemory, call: &Call) -> Result<Option<Pending>, MemFault> {
    if call.unit == NONE
        || call.permutation == 0
        || object_of_types(&*m, call.unit, UNIT_TYPES)? == 0
    {
        return Ok(None);
    }
    let def = definition(&*m, call.tag)?;
    let row = range_row(&*m, def, call.range as i32)?;
    let first = i16_at(&*m, row.wrapping_add(8))?;
    let perms = m.u32(m.u32(GESTALT)?.wrapping_add(0x2c))?;
    let index = ((call
        .permutation
        .wrapping_sub((first << 4) as u32)
        .wrapping_sub(perms)) as i32
        >> 4) as u32;
    if m.u8(def.wrapping_add(1))? & 2 != 0 {
        return Ok(None);
    }
    let class = sound_class(&*m, i32::from(m.u8(def.wrapping_add(2))? as i8))?;
    if (m.u8(class.wrapping_add(8))? >> 1) & 1 == 0 {
        return Ok(None);
    }
    if m.u16(def.wrapping_add(0xe))? == 0xffff {
        return Ok(None);
    }
    let extra = extra_info(&*m, def)?;
    if extra == 0 {
        return Ok(None);
    }
    let residency = residency(m, extra.wrapping_add(8))?;
    Ok(Some(Pending {
        index,
        extra,
        residency,
    }))
}

/// `0x218e50(eax = tag, dx = range, cx = permutation, tick)`: the
/// permutation's aperture at `tick`, f32 bits; 0 for a permutation out of
/// its range or one with no mouth curve.
pub fn aperture(
    m: &dyn GuestMemory,
    tag: u32,
    range: i16,
    permutation: i16,
    tick: i16,
) -> Result<u32, MemFault> {
    let def = definition(m, tag)?;
    let row = range_row(m, def, i32::from(range))?;
    let p = i32::from(permutation);
    let clamped = if p < 0 {
        0
    } else {
        p.min(i16_at(m, row.wrapping_add(0xa))? - 1)
    };
    if clamped != p {
        return Ok(0);
    }
    let gestalt = m.u32(GESTALT)?;
    let runtime = m.u32(extra_info(m, def)?.wrapping_add(4))?;
    let record = m
        .u32(gestalt.wrapping_add(0x2c))?
        .wrapping_add((i16_at(m, row.wrapping_add(8))? + p).wrapping_mul(16) as u32);
    let descriptor = m
        .u32(runtime.wrapping_add(0xc))?
        .wrapping_add((i32::from(m.u8(record.wrapping_add(5))? as i8) << 4) as u32);
    let length = m.i32(descriptor.wrapping_add(4))?;
    if length <= 0 {
        return Ok(0);
    }
    let t = i32::from(tick);
    let t = if t < 0 {
        0
    } else if t > length - 1 {
        length - 1
    } else {
        t
    };
    let at = m
        .u32(runtime.wrapping_add(4))?
        .wrapping_add(i32::from(t as i16) as u32)
        .wrapping_add(m.u32(descriptor)?);
    let byte = m.u8(at)?;
    Ok(Fp::guest(MXCSR).mul(det::from_int(i32::from(byte)), BYTE))
}

/// `0x18c9b0(eax = unit, xmm0 = aperture)`: with dialogue on, the unit's
/// aperture moves toward `target` by at most 0.3.
pub fn step_aperture(m: &mut dyn GuestMemory, unit: u32, target: u32) -> Result<(), MemFault> {
    if m.u8(m.u32(DIALOGUE)?.wrapping_add(DIALOGUE_ON))? == 0
        || object_of_types(&*m, unit, UNIT_TYPES)? == 0
    {
        return Ok(());
    }
    let at = facial::object(&*m, unit)?.wrapping_add(APERTURE);
    let mut fp = Fp::guest(MXCSR);
    let now = m.u32(at)?;
    let d = fp.sub(target, now);
    let d = if det::comiss(STEP_DOWN, d) == Some(Ordering::Greater) {
        STEP_DOWN
    } else if det::comiss(d, STEP_UP) == Some(Ordering::Greater) {
        STEP_UP
    } else {
        d
    };
    m.set_u32(at, fp.add(now, d))
}

/// `0x18ca20(unit, tag, permutation, seconds)`: a permutation with a
/// facial sequence blends it into a speaking unit.
pub fn facial_sequence(
    m: &mut dyn GuestMemory,
    unit: u32,
    tag: u32,
    permutation: u32,
    seconds: u32,
) -> Result<(), MemFault> {
    let def = definition(&*m, tag)?;
    let runtime = m.u32(extra_info(&*m, def)?.wrapping_add(4))?;
    let descriptor = m
        .u32(runtime.wrapping_add(0xc))?
        .wrapping_add((i32::from(m.u8(permutation.wrapping_add(5))? as i8) << 4) as u32);
    let size = m.i32(descriptor.wrapping_add(0xc))?;
    if size <= 0 {
        return Ok(());
    }
    if m.u8(m.u32(DIALOGUE)?.wrapping_add(DIALOGUE_ON))? == 0
        || object_of_types(&*m, unit, UNIT_TYPES)? == 0
    {
        return Ok(());
    }
    let data = m
        .u32(runtime.wrapping_add(4))?
        .wrapping_add(m.u32(descriptor.wrapping_add(8))?);
    facial::unit_sequence(
        m,
        unit,
        tag,
        Curve {
            size: size as u32,
            data,
        },
        seconds,
    )
}

/// `0x18c720` past the residency answer: nothing for a resource that is
/// not resident unless its element holds data in the tag (`+0` above 0);
/// else the tick, the aperture and the facial sequence.
pub fn finish(
    m: &mut dyn GuestMemory,
    call: &Call,
    pending: &Pending,
    resident: bool,
) -> Result<(), MemFault> {
    if !resident && m.i32(pending.extra)? <= 0 {
        return Ok(());
    }
    let def = definition(&*m, call.tag)?;
    let row = range_row(&*m, def, call.range as i32)?;
    let scaled = Fp::guest(MXCSR).mul(call.seconds, TICKS);
    let x = X87::new(FCW);
    let tick = x.fist32(x.load32(scaled));
    let target = aperture(
        &*m,
        call.tag,
        call.range as i16,
        pending.index as i16,
        tick as i16,
    )?;
    step_aperture(m, call.unit, target)?;
    let perms = m.u32(m.u32(GESTALT)?.wrapping_add(0x2c))?;
    let record = perms.wrapping_add(
        (i16_at(&*m, row.wrapping_add(8))?.wrapping_add(pending.index as i32) << 4) as u32,
    );
    facial_sequence(m, call.unit, call.tag, record, call.seconds)
}

/// The whole of `0x18c6a0` when the residency needs no I/O; `Some` with
/// the pending mouth when the cache must answer first.
pub fn run(m: &mut dyn GuestMemory, call: &Call) -> Result<Option<Pending>, MemFault> {
    let Some(p) = gate(m, call)? else {
        return Ok(None);
    };
    match p.residency {
        Residency::Resident => finish(m, call, &p, true).map(|_| None),
        Residency::Absent => finish(m, call, &p, false).map(|_| None),
        Residency::Guest => Ok(Some(p)),
    }
}

#[cfg(test)]
mod tests;
