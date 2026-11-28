//! A start entry's request record read into the table's
//! [`Request`] (docs/engine/sound-instances.md, "Admission").
//!
//! The record and the four arguments are the ones `0x126000` reads: the
//! tag, a channel byte, the record, a promotion rule. Field by field the
//! reads are `0x126000`'s own, in its order. The
//! callback table at `+0x58` (flag bit 5) is classified into an
//! [`Owner`]: the unit and script line tables by their finish slot, the
//! start layer's two tables by address. A table that is none of them
//! refuses the request.

use engine::memory::GuestMemory;
use sim_sound::instances::{Handle, Request, SourceParams, TagIndex};
use sim_sound::owners::{Own, Owner};
use sim_sound::starts::{STARTED_TABLE, SOUND_TABLE};

/// Finish slot (`+0x10`) of the unit dialogue table: `0x18c8c0`.
pub const UNIT_FINISH: u32 = 0x0018_c8c0;
/// Finish slot of the script line table: `0x18c910`.
pub const SCRIPT_LINE_FINISH: u32 = 0x0018_c910;
const NONE: u32 = u32::MAX;

/// Why a record is not a request the table admits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// A read faulted at this address.
    Unreadable(u32),
    /// The callback table's finish is not one of the known tables
    /// (`sound.refused_unknown_owner`).
    UnknownOwner { table: u32, finish: u32 },
}

/// The record's fields in the order and widths the admission reads them.
pub fn read(m: &dyn GuestMemory, args: [u32; 4]) -> Result<Request, RequestError> {
    let [tag, channel, record, rule] = args;
    let u32_at = |a: u32| m.u32(a).map_err(|_| RequestError::Unreadable(a));
    let u16_at = |a: u32| m.u16(a).map_err(|_| RequestError::Unreadable(a));
    let u8_at = |a: u32| m.u8(a).map_err(|_| RequestError::Unreadable(a));
    let flags = u32_at(record)?;
    let has = |bit: u32| flags & (1 << bit) != 0;
    let mut source = SourceParams::default();
    for (i, word) in source.0.iter_mut().enumerate() {
        *word = u32_at(record.wrapping_add(12 + 4 * i as u32))?;
    }
    let owner = if has(5) { owner(m, u32_at(record.wrapping_add(0x58))?, record)? } else { Owner::None };
    Ok(Request {
        tag: TagIndex(tag),
        channel: channel as u8,
        rule: rule as i32,
        word_04: u16_at(record.wrapping_add(4))?,
        flags_08: if has(1) { Some(u32_at(record.wrapping_add(8))?) } else { None },
        source,
        owner,
        track: None,
        follows: None,
        gain_base: if has(7) { Some(f32::from_bits(u32_at(record.wrapping_add(0x94))?)) } else { None },
        selection: if has(10) {
            Some((u16_at(record.wrapping_add(0x9c))?, u16_at(record.wrapping_add(0x9e))?))
        } else {
            None
        },
        extra_8e: if has(6) { Some(u16_at(record.wrapping_add(0x8e))?) } else { None },
        extra_90: if has(9) { Some(u8_at(record.wrapping_add(0x90))?) } else { None },
        extra_98: if has(8) { Some(u32_at(record.wrapping_add(0x98))?) } else { None },
        level: 0.0,
        detail: false,
    })
}

/// Payload `+0x28` (record `+0x84`): the start a start layer table names.
pub const START_AT: u32 = 0x84;
/// Payload `+0x2c` (record `+0x88`): the table a start's sound had before
/// `0x21dc60` took it over, 0 for none.
pub const OWN_TABLE_AT: u32 = 0x88;

/// The owner a callback table names. The payload's first word is the
/// object (Not verified in the design page: the payload layout).
fn owner(m: &dyn GuestMemory, table: u32, record: u32) -> Result<Owner, RequestError> {
    let u32_at = |a: u32| m.u32(a).map_err(|_| RequestError::Unreadable(a));
    match table {
        STARTED_TABLE => return Ok(Owner::Started(Handle::from_raw(u32_at(record.wrapping_add(START_AT))?))),
        SOUND_TABLE => {
            let start = Handle::from_raw(u32_at(record.wrapping_add(START_AT))?);
            let own = match u32_at(record.wrapping_add(OWN_TABLE_AT))? {
                0 => Own::None,
                inner => match owner(m, inner, record)? {
                    Owner::Unit(object) => Own::Unit(object),
                    Owner::ScriptLine(object) => Own::ScriptLine(object),
                    _ => return Err(RequestError::UnknownOwner { table: inner, finish: 0 }),
                },
            };
            return Ok(Owner::Start { start, own });
        }
        _ => {}
    }
    let finish = u32_at(table.wrapping_add(0x10))?;
    let object = u32_at(record.wrapping_add(0x5c))?;
    match finish {
        UNIT_FINISH => Ok(Owner::Unit(object)),
        SCRIPT_LINE_FINISH => Ok(Owner::ScriptLine((object != NONE).then_some(object))),
        _ => Err(RequestError::UnknownOwner { table, finish }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const RECORD: u32 = 0x0060_0000;
    const TABLE: u32 = 0x0060_1000;

    fn guest(flags: u32, finish: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(RECORD, 0x2000);
        g.put_u32(RECORD, flags).put_u16(RECORD + 4, 0x1234).put_u32(RECORD + 8, 0xdead_beef);
        for i in 0..17 {
            g.put_u32(RECORD + 12 + 4 * i, 0x100 + i);
        }
        g.put_u32(RECORD + 0x58, TABLE).put_u32(RECORD + 0x5c, 0xe001_0007);
        g.put_u16(RECORD + 0x8e, 0x80).put_u8(RECORD + 0x90, 9).put_u32(RECORD + 0x94, 2.5f32.to_bits());
        g.put_u32(RECORD + 0x98, 77).put_u16(RECORD + 0x9c, 1).put_u16(RECORD + 0x9e, 2);
        g.put_u32(TABLE + 0x10, finish);
        g
    }

    #[test]
    fn a_request_record_reads_into_a_request() {
        let all = (1 << 1) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 9) | (1 << 10);
        let g = guest(all, UNIT_FINISH);
        let r = read(&g, [0xe100_0004, 0x1203, RECORD, 2]).unwrap();
        assert_eq!((r.tag, r.channel, r.rule, r.word_04), (TagIndex(0xe100_0004), 3, 2, 0x1234));
        assert_eq!(r.flags_08, Some(0xdead_beef));
        assert_eq!(r.source.0[16], 0x110);
        assert_eq!(r.owner, Owner::Unit(0xe001_0007));
        assert_eq!(r.gain_base, Some(2.5));
        assert_eq!(r.selection, Some((1, 2)));
        assert_eq!((r.extra_8e, r.extra_90, r.extra_98), (Some(0x80), Some(9), Some(77)));
        // No flags: every optional field absent, no owner.
        let r = read(&guest(0, 0), [1, 0, RECORD, u32::MAX]).unwrap();
        assert_eq!((r.rule, r.owner, r.flags_08, r.selection, r.gain_base), (-1, Owner::None, None, None, None));
        // A script line with no object.
        let mut g = guest(1 << 5, SCRIPT_LINE_FINISH);
        g.put_u32(RECORD + 0x5c, NONE);
        assert_eq!(read(&g, [1, 0, RECORD, 0]).unwrap().owner, Owner::ScriptLine(None));
    }

    #[test]
    fn the_start_layer_tables_name_a_start() {
        // `0x44a1e0`: a sound a start's component admitted.
        let mut g = guest(1 << 5, 0);
        g.put_u32(RECORD + 0x58, STARTED_TABLE).put_u32(RECORD + START_AT, 0xef73_0002);
        assert_eq!(read(&g, [1, 0, RECORD, 0]).unwrap().owner, Owner::Started(Handle::from_raw(0xef73_0002)));
        // `0x44a1c0`: a start's own sound, keeping the unit table it had.
        g.put_u32(RECORD + 0x58, SOUND_TABLE).put_u32(RECORD + OWN_TABLE_AT, TABLE).put_u32(TABLE + 0x10, UNIT_FINISH);
        let start = Handle::from_raw(0xef73_0002);
        assert_eq!(read(&g, [1, 0, RECORD, 0]).unwrap().owner, Owner::Start { start, own: Own::Unit(0xe001_0007) });
        // No table of its own.
        g.put_u32(RECORD + OWN_TABLE_AT, 0);
        assert_eq!(read(&g, [1, 0, RECORD, 0]).unwrap().owner, Owner::Start { start, own: Own::None });
        // A start layer table inside another is not a request's owner.
        g.put_u32(RECORD + OWN_TABLE_AT, STARTED_TABLE);
        assert_eq!(read(&g, [1, 0, RECORD, 0]), Err(RequestError::UnknownOwner { table: STARTED_TABLE, finish: 0 }));
    }

    #[test]
    fn an_unknown_callback_table_refuses_the_request() {
        let g = guest(1 << 5, 0x0012_3456);
        assert_eq!(
            read(&g, [1, 0, RECORD, 0]),
            Err(RequestError::UnknownOwner { table: TABLE, finish: 0x0012_3456 })
        );
        assert_eq!(read(&g, [1, 0, 0x10, 0]), Err(RequestError::Unreadable(0x10)));
    }
}
