//! The retail record paths answered natively for the script index
//! (`rules::SCRIPT_INDEX`; docs/game/game-types-without-carriers.md, "No
//! retail engine object"), so a record of it survives every machine's
//! clean-up, crosses the wire and gets the distributed protocol:
//!
//! - `VARIANT_CLEAN_UP` (`0x19d650`), a `before`: a script record is
//!   answered by `variants::record::clean_up`. Clean, it returns AL 1 with
//!   the registers of the game's own clean exit; with only a non-zero type
//!   area, the area is zeroed and it returns AL 0; with a broken common
//!   setting, the index becomes Slayer's and the game's body runs, which
//!   resets the record to its default Slayer record. Any other record goes
//!   to the game's body untouched.
//! - `VARIANT_PROTOCOL` (`0x161c90`), a `before`: every record is answered
//!   by `variants::record::protocol`, which is the game's table with the
//!   script index distributed, and leaves the registers the game's body
//!   leaves.
//! - `VARIANT_ENCODE` (`0x7cc50`) and `VARIANT_DECODE` (`0x7d520`),
//!   `before`s: a script record is written and read by `variants::wire`,
//!   with no type area on the wire and a zero one in the record. The
//!   game's encoder would jump through a zero entry for it and its decoder
//!   refuses it. Any other record, and a stream we cannot read whole, go
//!   to the game's body.
//! - `VARIANT_CATEGORY` (`0x216a50`), a `before`: a script record is filed
//!   under Slayer's content category (`variants::record::content_category`)
//!   on a save or a list add; the game's body would jump through a zero
//!   entry. Any other record goes to the game's body.
//!
//! The differential test against the lifted original is
//! `crates/rewrites/engine/tests/variant_record_differential.rs`.

use std::sync::Arc;

use engine::GuestMemory;
use engine::points;
use variants::record::{self, CleanUp, RECORD_SIZE};
use variants::wire::{self, Bits};

/// The clean-up's answer for the record at `at`, applied to guest memory:
/// `Some(al)` when it is answered here, `None` when the game's body runs.
pub fn clean_up(m: &mut dyn GuestMemory, at: u32) -> Result<Option<u8>, engine::MemFault> {
    let mut raw = [0u8; RECORD_SIZE];
    m.read(at, &mut raw)?;
    let was = raw;
    let verdict = record::clean_up(&mut raw);
    if raw != was {
        m.write(at, &raw)?;
    }
    Ok(match verdict {
        CleanUp::Retail | CleanUp::ResetAsSlayer => None,
        CleanUp::Clean => Some(1),
        CleanUp::TypeAreaCleared => Some(0),
    })
}

/// The largest stream read whole, in bytes: a parameters update's.
pub const MAX_STREAM: u32 = 0x1_0000;

/// The bit stream at `at` (`variants::wire::Bits`), or `None` when its
/// size is past [`MAX_STREAM`] or its position is outside it.
pub fn read_bits(m: &dyn GuestMemory, at: u32) -> Result<Option<Bits>, engine::MemFault> {
    let words = m.u32(at + wire::WORDS_AT)?;
    let size = m.u32(at + wire::SIZE_AT)?;
    let pos = m.u32(at + wire::POS_AT)?;
    if size > MAX_STREAM || pos > size * 8 {
        return Ok(None);
    }
    let bytes = m.bytes(words, size.div_ceil(4) as usize * 4)?;
    Ok(Some(Bits {
        words: bytes
            .chunks_exact(4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .collect(),
        size,
        pos,
        error: m.u8(at + wire::ERROR_AT)? != 0,
    }))
}

/// Puts `bits` back at `at`: the words that changed from `was`, the
/// position, and the error byte when it was set.
fn write_bits(
    m: &mut dyn GuestMemory,
    at: u32,
    was: &Bits,
    bits: &Bits,
) -> Result<(), engine::MemFault> {
    let words = m.u32(at + wire::WORDS_AT)?;
    for (i, (a, b)) in was.words.iter().zip(&bits.words).enumerate() {
        if a != b {
            m.set_u32(words + 4 * i as u32, *b)?;
        }
    }
    m.set_u32(at + wire::POS_AT, bits.pos)?;
    if bits.error && !was.error {
        m.set_u8(at + wire::ERROR_AT, 1)?;
    }
    Ok(())
}

fn is_script(engine: u32) -> bool {
    engine == record::SCRIPT_INDEX as u32
}

/// The encode for a script record (`record` at `rec`, the stream at
/// `writer`): `Some(())` when it was written here.
pub fn encode(
    m: &mut dyn GuestMemory,
    writer: u32,
    rec: u32,
) -> Result<Option<()>, engine::MemFault> {
    let mut raw = [0u8; RECORD_SIZE];
    m.read(rec, &mut raw)?;
    let engine = &raw[record::ENGINE_AT..record::ENGINE_AT + 4];
    if !is_script(u32::from_le_bytes(engine.try_into().unwrap())) {
        return Ok(None);
    }
    let Some(was) = read_bits(m, writer)? else {
        return Ok(None);
    };
    let mut bits = was.clone();
    if wire::encode(&raw, &mut bits).is_err() {
        return Ok(None);
    }
    write_bits(m, writer, &was, &bits)?;
    Ok(Some(()))
}

/// The decode of a script record (into `rec`, from the stream at
/// `reader`): `Some(al)` when it was read here.
pub fn decode(
    m: &mut dyn GuestMemory,
    rec: u32,
    reader: u32,
) -> Result<Option<u8>, engine::MemFault> {
    let Some(was) = read_bits(m, reader)? else {
        return Ok(None);
    };
    if !is_script(wire::peek_engine(&was)) {
        return Ok(None);
    }
    let mut out = [0u8; RECORD_SIZE];
    m.read(rec, &mut out)?;
    let mut bits = was.clone();
    let answer = wire::decode(&mut out, &mut bits);
    m.write(rec, &out)?;
    write_bits(m, reader, &was, &bits)?;
    Ok(Some(u8::from(answer == wire::Decoded::Kept)))
}

/// The hooks. No other feature hooks these points. The content
/// category is answered for the script index only; the protocol map for
/// every index.
pub fn register(b: &mut hooks::Builder) {
    use hooks::Flow;
    b.raw().before(
        &points::VARIANT_CLEAN_UP,
        "rules: the record clean-up keeps a script record",
        move |c| {
            let answer = {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let m: &mut dyn GuestMemory = &mut *mem;
                // A record we cannot read is the game's to fault on.
                clean_up(m, c.cpu.ebx).ok().flatten()
            };
            let Some(al) = answer else {
                return Ok(Flow::Continue);
            };
            let [_, ecx, edx] = record::CLEAN_UP_EXIT;
            c.cpu.eax = u32::from(al);
            c.cpu.ecx = ecx;
            c.cpu.edx = edx;
            c.ret(0).map(Flow::Return)
        },
    );
    b.raw().before(
        &points::VARIANT_ENCODE,
        "rules: a script record goes on the wire with no type area",
        move |c| {
            let (Some(writer), Some(rec)) = (c.arg_u32("writer"), c.arg_u32("record")) else {
                return Ok(Flow::Continue);
            };
            let done = {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let m: &mut dyn GuestMemory = &mut *mem;
                encode(m, writer, rec).ok().flatten()
            };
            if done.is_none() {
                return Ok(Flow::Continue);
            }
            c.ret(8).map(Flow::Return)
        },
    );
    b.raw().before(
        &points::VARIANT_DECODE,
        "rules: a script record comes off the wire with a zero type area",
        move |c| {
            let answer = {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let m: &mut dyn GuestMemory = &mut *mem;
                decode(m, c.cpu.eax, c.cpu.ecx).ok().flatten()
            };
            let Some(al) = answer else {
                return Ok(Flow::Continue);
            };
            let [_, ecx, edx] = record::CLEAN_UP_EXIT;
            c.cpu.eax = u32::from(al);
            c.cpu.ecx = ecx;
            c.cpu.edx = edx;
            c.ret(0).map(Flow::Return)
        },
    );
    b.raw().before(
        &points::VARIANT_PROTOCOL,
        "rules: a script record plays distributed",
        move |c| {
            let engine = {
                let rt = Arc::clone(c.ctx.runtime());
                let mem = rt.memory();
                let m: &dyn GuestMemory = &*mem;
                m.i32(c.cpu.ecx.wrapping_add(record::ENGINE_AT as u32)).ok()
            };
            let Some(engine) = engine else {
                return Ok(Flow::Continue);
            };
            let (eax, ecx, edx) = record::protocol(engine);
            c.cpu.eax = eax;
            c.cpu.ecx = ecx;
            if let Some(edx) = edx {
                c.cpu.edx = edx;
            }
            c.ret(0).map(Flow::Return)
        },
    );
    b.raw().before(
        &points::VARIANT_CATEGORY,
        "rules: a script record is filed as Slayer's",
        move |c| {
            let engine = {
                let rt = Arc::clone(c.ctx.runtime());
                let mem = rt.memory();
                let m: &dyn GuestMemory = &*mem;
                m.i32(c.cpu.eax.wrapping_add(record::ENGINE_AT as u32)).ok()
            };
            // Only the script index: every other index is the game's body.
            let Some(category) = engine
                .filter(|&e| e == record::SCRIPT_INDEX)
                .and_then(record::content_category)
            else {
                return Ok(Flow::Continue);
            };
            c.cpu.eax = category;
            c.cpu.ecx = (record::SCRIPT_INDEX as u32).wrapping_sub(1);
            c.ret(0).map(Flow::Return)
        },
    );
}
