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
//! - `GAME_ACTIVATE`, a `before` in the last class: the record about to
//!   start moves onto the script index when it would play beside or in
//!   place of a script ([`move_onto_script_index`]).
//!
//! The differential test against the lifted original is
//! `crates/rewrites/engine/tests/variant_record_differential.rs`.

use std::sync::Arc;

use guest_layout::GuestMemory;
use guest_layout::points;
use variants::record::{self, CleanUp, RECORD_SIZE};
use variants::wire::{self, Bits};

/// The clean-up's answer for the record at `at`, applied to guest memory:
/// `Some(al)` when it is answered here, `None` when the game's body runs.
pub fn clean_up(m: &mut dyn GuestMemory, at: u32) -> Result<Option<u8>, guest_layout::MemFault> {
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
pub fn read_bits(m: &dyn GuestMemory, at: u32) -> Result<Option<Bits>, guest_layout::MemFault> {
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
) -> Result<(), guest_layout::MemFault> {
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
) -> Result<Option<()>, guest_layout::MemFault> {
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
) -> Result<Option<u8>, guest_layout::MemFault> {
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

/// A multiplayer game's options at `options`: the record to start is at
/// `options + variant_ext::OPTIONS_VARIANT`, and the mode at `+0`.
const MODE_MULTIPLAYER: u32 = crate::skulls_mp::MODE_MULTIPLAYER;

/// What the swap did to a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Swap {
    /// The record stays as it is.
    Stayed,
    /// The record moved onto the script index.
    Moved,
    /// A retail record that cannot map onto its built-in's options (a
    /// value the option's range lacks); it stays as it is.
    Refused(String),
}

/// The index-6 record of a record that names its type's own built-in
/// (a `uses` script's or the matchmaking CTF's): the same mapping as a
/// record with no script (`variants::portable::script_record_of_retail`),
/// so the record's type-area settings still reach the built-in.
fn builtin_record(raw: &[u8; RECORD_SIZE]) -> Result<Option<variants::GameVariant>, String> {
    use variants::ext::{Block, Mode};
    let Ok(v) = variants::GameVariant::from_bytes(raw) else {
        return Ok(None);
    };
    let Block::Ext(mut e) = v.ext_state() else {
        return Ok(None);
    };
    let src = u8::try_from(v.game_type().engine()).ok().and_then(::rules::builtin::builtin);
    let Some(src) = src.filter(|_| e.script.mode == Mode::Rules) else {
        return Ok(None);
    };
    let Ok(ir) = ::rules::compile(src) else {
        return Ok(None);
    };
    if e.script.prefix[..] != ir.id().0[..8] {
        return Ok(None);
    }
    e.script.mode = Mode::None;
    let mut plain = v;
    plain.set_ext(&e).map_err(|e| e.to_string())?;
    variants::portable::script_record_of_retail(&plain).map_err(|e| e.to_string())
}

/// Before a multiplayer game starts, the record at `options` moves onto the
/// script index when a retail engine would otherwise run beside its script
/// or in its place, so each game has one scorer (the script):
///
/// - a record with no script on CTF, Oddball, King of the Hill,
///   Juggernaut, Territories or Assault becomes the index-6 record of that
///   type's built-in, its type area mapped into the built-in's options
///   (`variants::portable::script_record_of_retail`); a record that
///   cannot map is [`Swap::Refused`] and plays on its retail engine;
/// - a record that names its own type's built-in maps the same way;
/// - any other record that names a script, on any retail index, takes the
///   script index with its type area dropped
///   ([`record::onto_script_index`]).
///
/// Every machine does it from the record it was given, so each starts the
/// same engine. Only the copy in the options changes; the stored record
/// and the wire keep their bytes.
pub fn move_onto_script_index(
    m: &mut dyn GuestMemory,
    options: u32,
) -> Result<Swap, guest_layout::MemFault> {
    if m.u32(options)? != MODE_MULTIPLAYER {
        return Ok(Swap::Stayed);
    }
    let at = options.wrapping_add(crate::variant_ext::OPTIONS_VARIANT);
    let mut raw = [0u8; RECORD_SIZE];
    m.read(at, &mut raw)?;
    // The Forge row opens Custom Game's lobby with its plain Slayer
    // template. Leave the lobby record intact for a return to Custom
    // Games, but give an editing session no Slayer scorer or round clock.
    let source = variants::GameVariant::from_bytes(&raw).expect("complete variant record");
    if crate::lobby_start::mode() == crate::lobby_start::Mode::Forge
        && source.game_type() == variants::GameType::Slayer
        && source.ext().script.mode == variants::ext::Mode::None
    {
        raw[record::ENGINE_AT..record::ENGINE_AT + 4]
            .copy_from_slice(&record::SCRIPT_INDEX.to_le_bytes());
        raw[record::TYPE_AREA].fill(0);
        let mut editor = variants::GameVariant::from_bytes(&raw).expect("complete variant record");
        editor.set_name("Forge").expect("short name");
        for id in [1, 10] {
            editor.set(variants::SettingId(id), 0).expect("unlimited score and time");
        }
        m.write(at, editor.bytes())?;
        return Ok(Swap::Moved);
    }
    let mapped = variants::portable::script_record_of_retail(&source).map_err(|e| e.to_string());
    let mapped = match mapped {
        Ok(None) => builtin_record(&raw),
        other => other,
    };
    match mapped {
        Ok(Some(out)) => raw = out.to_bytes(),
        Ok(None) => {
            if !record::onto_script_index(&mut raw) {
                return Ok(Swap::Stayed);
            }
        }
        Err(why) => return Ok(Swap::Refused(why)),
    }
    m.write(at, &raw)?;
    Ok(Swap::Moved)
}

/// Logs a refused swap's reason once per distinct reason.
fn log_refused_once(why: &str) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Mutex::default);
    if seen.lock().is_ok_and(|mut s| s.insert(why.to_string())) {
        eprintln!("rules: the record stays on its retail engine: {why}");
    }
}

/// The hooks. No other feature hooks these points. The content
/// category is answered for the script index only; the protocol map for
/// every index.
pub fn register(b: &mut hooks::Builder) {
    use hooks::Flow;
    // After the other start hooks, as the engine object's: the Live
    // guest's block from its host is written first, and the engine start
    // after them all reads the record.
    b.raw()
        .before(
            &points::GAME_ACTIVATE,
            "rules: a record that scores beside its engine starts on the script index",
            move |c| {
                let options = c.cpu.eax;
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let m: &mut dyn GuestMemory = &mut *mem;
                match move_onto_script_index(m, options) {
                    Ok(Swap::Moved) => eprintln!("rules: the record starts on the script index"),
                    Ok(Swap::Refused(why)) => log_refused_once(&why),
                    _ => {}
                }
                Ok(Flow::Continue)
            },
        )
        .last()
        .after_feature(crate::skulls_mp::FEATURE);
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
