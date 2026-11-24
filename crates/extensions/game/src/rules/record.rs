//! The two retail record paths answered natively for the script index
//! (`rules::SCRIPT_INDEX`; docs/game/game-types-without-carriers.md, "No
//! retail engine object"), so a record of it survives every machine's
//! clean-up and gets the distributed protocol:
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
//!
//! The differential test against the lifted original is
//! `crates/rewrites/engine/tests/variant_record_differential.rs`.

use std::sync::Arc;

use engine::GuestMemory;
use engine::points;
use variants::record::{self, CleanUp, RECORD_SIZE};

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

/// The hooks. No other feature hooks these points.
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
}
