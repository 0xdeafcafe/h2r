//! The engine's sound instance as the sim reads it: what a start left in
//! the pool, how long its permutation runs, the lip-sync update the mixer's
//! loop makes for its owner, and the stop ramp's finish rule
//! (docs/engine/sound-sim.md, "Instances").
//!
//! The sim does not make instances: the engine's own admission
//! (`SOUND_IMPULSE_ALLOCATE`, `0x126000`) does, with its random draws and
//! its rejections, and the sim reads the instance it returned.

use guest_layout::{GuestMemory, points};

pub const NONE: u32 = u32::MAX;
/// Table `+8`: the lip-sync update the mixer's loop calls (`0x129593`).
pub const UPDATE_LIP_SYNC: u32 = 0x18c6a0;
/// The finish of an instance (`0x127320`: instance in EAX, reason on the
/// stack): its owner's callback, its channel and its record.
pub const FINISH: u32 = 0x0012_7320;
/// The ramp function (`0x12a810`: instance in EAX): the instance's current
/// ramp value in dB as an `f32`, in EAX.
pub const RAMP: u32 = 0x0012_a810;
/// The reason of a finish that the fade of a stop completes (`0x1294b7`).
pub const FADE_REASON: u32 = 4;
/// The reason of a finish for a sound that played out. The per-instance
/// service (`0x128040`) finishes an instance whose channel has stopped
/// (`[channel + 9] == 0`, or no channel) with 2 when its flags word (`+4`)
/// has bits 9 and 6 and its source is not a loop (`[source + 0xd] & 7 == 0`,
/// `0x1280d4..0x12810f`). The other reasons there are 0xF (`0x128146`), 3
/// (the owner's `+0` callback returned false, `0x12815d`), and the start
/// failures 0xD, 0xE and 7.
pub const ENDED_REASON: u32 = 2;

const GESTALT: u32 = 0x0051_ebd4;
/// The mixer's globals: `+0x84` is its clock in milliseconds.
pub const MIXER: u32 = 0x004e_6380;
pub const MIXER_CLOCK: u32 = 0x84;
/// The pool: `[pool + 0x38]` slots of `[pool + 0x24]` bytes from
/// `[pool + 0x44]`, each led by its salt.
const POOL_COUNT: u32 = 0x38;
const POOL_SIZE: u32 = 0x24;
const POOL_RECORDS: u32 = 0x44;
/// The fade floor: a ramp at or under it silences the sound (`0x44a0b4`).
pub(crate) const FLOOR: u32 = 0x0044_a0b4;

mod at {
    pub const FLAGS: u32 = 4;
    pub const TAG: u32 = 0xc;
    pub const OWNER: u32 = 0x10;
    pub const TABLE: u32 = 0x14;
    pub const GAIN: u32 = 0x20;
    pub const PAYLOAD: u32 = 0x5c;
    pub const RANGE: u32 = 0x9c;
    pub const PERMUTATION: u32 = 0x9d;
    pub const FADE_END: u32 = 0xb4;
    pub const FADE_START: u32 = 0xb8;
}
/// Flags bit 5: the owner is not called. Bit 7: a ramp is running.
const FLAG_NO_OWNER: u8 = 1 << 5;
const FLAG_RAMP: u8 = 1 << 7;
const TICKS_PER_SECOND: f32 = 30.0;

fn word(m: &dyn GuestMemory, a: u32) -> Option<u32> {
    m.u32(a).ok()
}

/// What the sim keeps of an instance, read once at its start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admitted {
    pub tag: u32,
    pub owner: u32,
    /// The length of the chosen permutation in milliseconds.
    pub duration_ms: Option<u32>,
}

/// The address of an instance's record, if the handle is a live slot.
pub fn record(m: &dyn GuestMemory, handle: u32) -> Option<u32> {
    if handle == NONE {
        return None;
    }
    let pool = word(m, points::SOUND_INSTANCES.base)?;
    let index = handle & 0xffff;
    if index >= word(m, pool.wrapping_add(POOL_COUNT))? {
        return None;
    }
    let size = word(m, pool.wrapping_add(POOL_SIZE))?;
    let at = word(m, pool.wrapping_add(POOL_RECORDS))?.wrapping_add(size.wrapping_mul(index));
    let salt = m.u16(at).ok()? as i16 as i32;
    (salt != 0 && salt == (handle as i32 >> 16)).then_some(at)
}

/// The instance the engine's admission returned.
pub fn read_admitted(m: &dyn GuestMemory, handle: u32) -> Option<Admitted> {
    let rec = record(m, handle)?;
    let tag = word(m, rec.wrapping_add(at::TAG))?;
    let owner = word(m, rec.wrapping_add(at::OWNER))?;
    let range = m.u8(rec.wrapping_add(at::RANGE)).ok()? as i8;
    let permutation = m.u8(rec.wrapping_add(at::PERMUTATION)).ok()? as i8;
    Some(Admitted { tag, owner, duration_ms: permutation_ms(m, tag, range, permutation) })
}

/// `0x218c60`: the permutation's length in milliseconds, the `u16` at its
/// `+6`, for a pitch range and permutation relative to the tag's first
/// range and the range's first permutation.
pub fn permutation_ms(m: &dyn GuestMemory, tag: u32, range: i8, permutation: i8) -> Option<u32> {
    let entries = word(m, points::TAG_ENTRIES.base)?;
    let data = word(m, entries.wrapping_add((tag & 0xffff) * 0x10 + 8)).filter(|&d| d != 0)?;
    let first_range = m.u16(data.wrapping_add(8)).ok()? as i16 as i32;
    let gestalt = word(m, GESTALT)?;
    let ranges = word(m, gestalt.wrapping_add(0x24))?;
    let perms = word(m, gestalt.wrapping_add(0x2c))?;
    let at_range = ranges.wrapping_add((first_range + i32::from(range)) as u32 * 12);
    let first = m.u16(at_range.wrapping_add(8)).ok()? as i16 as i32;
    let at_perm = perms.wrapping_add((first + i32::from(permutation)) as u32 * 16);
    m.u16(at_perm.wrapping_add(6)).ok().map(u32::from)
}

/// The mixer loop's call of the owner's `+8` (`0x129593`) for an instance:
/// `(function, arguments)` when its table's update is the lip-sync update.
/// The permutation is the record the instance names; the time is the sim's.
pub fn update_call(m: &dyn GuestMemory, handle: u32, seconds: f32) -> Option<(u32, [u32; 6])> {
    let rec = record(m, handle)?;
    if m.u8(rec.wrapping_add(at::FLAGS)).ok()? & FLAG_NO_OWNER != 0 {
        return None;
    }
    let table = word(m, rec.wrapping_add(at::TABLE)).filter(|&t| t != 0)?;
    let function = word(m, table.wrapping_add(8)).filter(|&f| f == UPDATE_LIP_SYNC)?;
    let tag = word(m, rec.wrapping_add(at::TAG))?;
    let range = m.u8(rec.wrapping_add(at::RANGE)).ok()? as i8;
    let permutation = m.u8(rec.wrapping_add(at::PERMUTATION)).ok()? as i8;
    let entries = word(m, points::TAG_ENTRIES.base)?;
    let data = word(m, entries.wrapping_add((tag & 0xffff) * 0x10 + 8)).filter(|&d| d != 0)?;
    let first_range = m.u16(data.wrapping_add(8)).ok()? as i16 as i32;
    let gestalt = word(m, GESTALT)?;
    let ranges = word(m, gestalt.wrapping_add(0x24))?;
    let perms = word(m, gestalt.wrapping_add(0x2c))?;
    let first = m.u16(ranges.wrapping_add((first_range + i32::from(range)) as u32 * 12 + 8)).ok()? as i16 as i32;
    let at_perm = perms.wrapping_add((first + i32::from(permutation)) as u32 * 16);
    Some((
        function,
        [
            word(m, rec.wrapping_add(at::OWNER))?,
            rec.wrapping_add(at::PAYLOAD),
            tag,
            i32::from(range) as u32,
            at_perm,
            seconds.to_bits(),
        ],
    ))
}

/// Whether an instance is in a ramp (a stop started one).
pub fn ramping(m: &dyn GuestMemory, handle: u32) -> bool {
    record(m, handle)
        .and_then(|rec| m.u8(rec.wrapping_add(at::FLAGS)).ok())
        .is_some_and(|f| f & FLAG_RAMP != 0)
}

/// The mixer loop's test that finishes a ramping instance (`0x12948c`):
/// its gain plus the ramp value, `ramp` (the bits `0x12a810` returned),
/// is at or under the floor, and the ramp's end is past its start.
pub fn ramp_done(m: &dyn GuestMemory, handle: u32, ramp: u32) -> bool {
    let Some(rec) = record(m, handle) else { return false };
    let read = |a: u32| word(m, a);
    let (Some(gain), Some(floor), Some(end), Some(start), Some(flags)) = (
        read(rec.wrapping_add(at::GAIN)),
        read(FLOOR),
        read(rec.wrapping_add(at::FADE_END)),
        read(rec.wrapping_add(at::FADE_START)),
        m.u8(rec.wrapping_add(at::FLAGS)).ok(),
    ) else {
        return false;
    };
    let total = f32::from_bits(gain) + f32::from_bits(ramp);
    // `comiss floor, total; jb skip`: skip when the floor is below the total.
    let under = !(f32::from_bits(floor) < total);
    under && flags & FLAG_RAMP != 0 && (end as i32) > (start as i32)
}

/// The seconds since a start, from ticks. The engine's value is the
/// channel's `+0x10`, which `0x12a450` adds to once per service run
/// (`0x12a5c6`): the playback ratio of the channel's format (`0x21f650`)
/// times the run's wall-clock step (`[mixer + 0x1e0]`, from the timer
/// `0x3314b0` at `0x126a34`). The sim has no wall-clock step, so this is the
/// sim time elapsed, which is the engine's value at ratio 1.
pub fn seconds(ticks: u32) -> f32 {
    ticks as f32 / TICKS_PER_SECOND
}

/// A guest with one instance for the tests of this module and its parent.
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use guest_layout::testing::FakeGuest;
    pub(crate) use super::FLOOR;

    pub const POOL: u32 = 0x0060_0000;
    pub const RECORDS: u32 = 0x0060_1000;
    pub const ENTRIES: u32 = 0x0060_2000;
    pub const TAGDATA: u32 = 0x0060_3000;
    pub const GEST: u32 = 0x0060_4000;
    pub const RANGES: u32 = 0x0060_5000;
    pub const PERMS: u32 = 0x0060_6000;
    pub const MIXER_AT: u32 = 0x0060_8000;
    pub const TIME: u32 = 0x0060_9000;
    pub const TABLE: u32 = 0x0060_7000;
    pub const TAG: u32 = 0xe1e1_0003;
    /// Slot 2, salt 0x1234.
    pub const HANDLE: u32 = 0x1234_0002;
    pub const REC: u32 = RECORDS + 2 * 0xbc;

    pub fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x0044_0000, 0x1000).map(0x0051_0000, 0x1_0000).map(0x004e_0000, 0x1_0000).map(0x0060_0000, 0x1_0000);
        g.put_u32(points::SOUND_INSTANCES.base, POOL).put_u32(POOL + 0x38, 4).put_u32(POOL + 0x24, 0xbc).put_u32(POOL + 0x44, RECORDS);
        g.put_u16(REC, 0x1234);
        g.put_u32(REC + at::TAG, TAG).put_u32(REC + at::OWNER, 0xaaaa_0001).put_u32(REC + at::TABLE, TABLE);
        g.put_u8(REC + at::RANGE, 1).put_u8(REC + at::PERMUTATION, 2);
        // The tag: first pitch range 3; range 3 + 1 holds permutations from 5.
        g.put_u32(points::TAG_ENTRIES.base, ENTRIES).put_u32(ENTRIES + 3 * 0x10 + 8, TAGDATA).put_u16(TAGDATA + 8, 3);
        g.put_u32(GESTALT, GEST).put_u32(GEST + 0x24, RANGES).put_u32(GEST + 0x2c, PERMS);
        g.put_u16(RANGES + 4 * 12 + 8, 5);
        // Permutation 5 + 2 runs 1741 ms.
        g.put_u16(PERMS + 7 * 16 + 6, 1741);
        g.put_u32(TABLE + 8, UPDATE_LIP_SYNC);
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::fixture::*;

    #[test]
    fn a_handle_names_a_record_only_by_its_slot_and_salt() {
        let g = guest();
        assert_eq!(record(&g, HANDLE), Some(REC));
        assert_eq!(record(&g, 0x1235_0002), None, "another salt");
        assert_eq!(record(&g, 0x1234_0009), None, "past the pool");
        assert_eq!(record(&g, NONE), None);
    }

    #[test]
    fn the_admitted_instance_gives_its_tag_owner_and_permutation_length() {
        let g = guest();
        assert_eq!(
            read_admitted(&g, HANDLE),
            Some(Admitted { tag: TAG, owner: 0xaaaa_0001, duration_ms: Some(1741) })
        );
    }

    #[test]
    fn the_update_is_the_mixers_six_arguments() {
        let mut g = guest();
        let (f, args) = update_call(&g, HANDLE, 0.5).unwrap();
        assert_eq!(f, UPDATE_LIP_SYNC);
        assert_eq!(args, [0xaaaa_0001, REC + at::PAYLOAD, TAG, 1, PERMS + 7 * 16, 0.5f32.to_bits()]);
        // Another table function, or the owner switched off: no call.
        g.put_u32(TABLE + 8, 0x1234);
        assert_eq!(update_call(&g, HANDLE, 0.5), None);
        let mut g = guest();
        g.put_u8(REC + at::FLAGS, FLAG_NO_OWNER);
        assert_eq!(update_call(&g, HANDLE, 0.5), None);
    }

    #[test]
    fn a_ramp_is_done_at_the_floor_with_its_end_past_its_start() {
        let mut g = guest();
        g.put_u32(FLOOR, (-64.0f32).to_bits());
        g.put_u32(REC + at::GAIN, 0.0f32.to_bits());
        g.put_u8(REC + at::FLAGS, FLAG_RAMP).put_u32(REC + at::FADE_END, 400).put_u32(REC + at::FADE_START, 100);
        assert!(ramping(&g, HANDLE));
        assert!(!ramp_done(&g, HANDLE, (-10.0f32).to_bits()), "still audible");
        assert!(ramp_done(&g, HANDLE, (-64.0f32).to_bits()), "at the floor");
        assert!(ramp_done(&g, HANDLE, (-90.0f32).to_bits()));
        // The gain adds to the ramp.
        g.put_u32(REC + at::GAIN, (-30.0f32).to_bits());
        assert!(ramp_done(&g, HANDLE, (-40.0f32).to_bits()));
        // An end that is not past the start, or no ramp flag, is not done.
        g.put_u32(REC + at::FADE_END, 100);
        assert!(!ramp_done(&g, HANDLE, (-90.0f32).to_bits()));
        g.put_u32(REC + at::FADE_END, 400).put_u8(REC + at::FLAGS, 0);
        assert!(!ramp_done(&g, HANDLE, (-90.0f32).to_bits()));
        assert!(!ramping(&g, HANDLE));
    }
}
