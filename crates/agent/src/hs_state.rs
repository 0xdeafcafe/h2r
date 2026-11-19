//! `hs.threads` and `hs.globals` (docs/runtime/agent-api.md, "state"):
//! the running game's HaloScript state, so "has the courtyard fight
//! started" is a global's value and "did the opening script run" a
//! thread's wake, not guesses.
//!
//! Both are game methods, answered at a tick boundary with guest memory at
//! hand. The layout is the interpreter's own (docs/engine/hs_vm.md, all
//! **E**): the thread array at `0x4F9384` (elements at the header's
//! `+0x44`, `0x418` each: category `+2`, script index `+4`, wake `+8`,
//! `-1` finished, `-2` dormant; the first `u16` is the datum salt, zero
//! for a free slot; 320 slots), the scenario globals' cells at `0x4F9380`
//! (elements at `+0x44`, `0x8` each, the cell at `+4`, one per global in
//! table order, as `hs_new_map` allocates them), and the scenario's script
//! and global definitions through the game globals `0x4E0350`: the scripts
//! at `+0x1BC` and their count at `+0x1B8` (`0x28` each: name, kind
//! `+0x20`, return type `+0x22`), the globals at `+0x1C4` and their count
//! at `+0x1C0` (`0x28` each: name, type `+0x20`).

use serde_json::{Value, json};

use crate::{Access, Agent};
use crate::exec::GameCtx;
use agent_proto::{RpcError, code};

/// The guest addresses, with the evidence in the module doc.
mod addr {
    /// The game globals: the script tables hang off it.
    pub const GAME_GLOBALS: u32 = 0x004e_0350;
    /// The scenario's scripts: the count at `+0x1B8`, the elements at
    /// `+0x1BC`.
    pub const SCRIPTS: u32 = 0x1b8;
    /// The scenario's globals: the count at `+0x1C0`, the elements at
    /// `+0x1C4`.
    pub const GLOBALS: u32 = 0x1c0;
    /// The script thread array.
    pub const THREADS: u32 = 0x004f_9384;
    /// The scenario globals' cells.
    pub const CELLS: u32 = 0x004f_9380;
    /// A datum array's elements sit at its header's `+0x44`.
    pub const ELEMENTS: u32 = 0x44;
    /// Bytes per thread record.
    pub const THREAD_LEN: u32 = 0x418;
    /// The interpreter's thread array holds this many slots
    /// (`0x00208ff0`).
    pub const THREAD_SLOTS: u32 = 320;
    /// Bytes per script or global definition.
    pub const DEF_LEN: u32 = 0x28;
}

/// The names of the script kinds (the interpreter's table, `0x0046fff8`).
const KINDS: [&str; 6] = [
    "startup",
    "dormant",
    "continuous",
    "static",
    "stub",
    "command",
];

/// The value types an agent cares to see named (the interpreter's table,
/// `0x0046ff00`); the rest report their index.
fn type_name(ty: u16) -> String {
    match ty {
        3 => "passthrough".into(),
        4 => "void".into(),
        5 => "boolean".into(),
        6 => "real".into(),
        7 => "short".into(),
        8 => "long".into(),
        9 => "string".into(),
        10 => "script".into(),
        11 => "string_id".into(),
        _ => format!("type {ty}"),
    }
}

/// Registers `hs.threads` and `hs.globals` on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method("hs.threads", Access::Read, threads);
    agent.add_game_method("hs.globals", Access::Read, globals);
}

type R = Result<Value, RpcError>;

fn unavailable(what: &str, e: engine::memory::MemFault) -> RpcError {
    RpcError::new(
        code::UNAVAILABLE,
        format!("hs: {what} unreadable ({e}); is a scenario running?"),
    )
}

/// Every live script thread: its script, kind, wake tick and slot.
fn threads(_v: Value, ctx: &mut GameCtx<'_>) -> R {
    let mut out = Vec::new();
    ctx.reader.with_guest(|m| -> Result<(), RpcError> {
        let g = m.u32(addr::GAME_GLOBALS).map_err(|e| unavailable("game globals", e))?;
        let (count, scripts) = (
            m.u32(g + addr::SCRIPTS).map_err(|e| unavailable("scripts", e))?,
            m.u32(g + addr::SCRIPTS + 4).map_err(|e| unavailable("scripts", e))?,
        );
        let name = |i: u32| -> Option<String> {
            if i >= count {
                return None;
            }
            let at = scripts + i * addr::DEF_LEN;
            let mut b = [0u8; 32];
            m.read(at, &mut b).ok()?;
            let end = b.iter().position(|&c| c == 0).unwrap_or(32);
            Some(String::from_utf8_lossy(&b[..end]).into_owned())
        };
        let threads = m
            .u32(m.u32(addr::THREADS).map_err(|e| unavailable("threads", e))? + addr::ELEMENTS)
            .map_err(|e| unavailable("threads", e))?;
        for slot in 0..addr::THREAD_SLOTS {
            let at = threads + slot * addr::THREAD_LEN;
            let salt = match m.u16(at) {
                Ok(s) => s,
                Err(_) => break,
            };
            if salt == 0 {
                continue; // A free slot.
            }
            let category = m.u8(at + 2).unwrap_or(0);
            let script = m.u16(at + 4).unwrap_or(0xffff);
            let wake = m.i32(at + 8).unwrap_or(-3);
            out.push(json!({
                "script": name(u32::from(script)).unwrap_or_else(|| format!("script {script}")),
                "kind": KINDS.get(m.u16(scripts + u32::from(script) * addr::DEF_LEN + 0x20)
                    .map_or(usize::MAX, usize::from))
                    .copied().unwrap_or("kind ?"),
                "category": category,
                "wake": wake,
                "slot": slot,
            }));
        }
        Ok(())
    })?;
    Ok(json!({ "threads": out }))
}

/// Every scenario global: its name, type and value.
fn globals(_v: Value, ctx: &mut GameCtx<'_>) -> R {
    let mut out = Vec::new();
    ctx.reader.with_guest(|m| -> Result<(), RpcError> {
        let g = m.u32(addr::GAME_GLOBALS).map_err(|e| unavailable("game globals", e))?;
        let count = m.u32(g + addr::GLOBALS).map_err(|e| unavailable("globals", e))?;
        let defs = m.u32(g + addr::GLOBALS + 4).map_err(|e| unavailable("globals", e))?;
        let cells = m
            .u32(m.u32(addr::CELLS).map_err(|e| unavailable("global cells", e))? + addr::ELEMENTS)
            .map_err(|e| unavailable("global cells", e))?;
        for i in 0..count {
            let at = defs + i * addr::DEF_LEN;
            let mut b = [0u8; 32];
            m.read(at, &mut b).map_err(|e| unavailable("global names", e))?;
            let end = b.iter().position(|&c| c == 0).unwrap_or(32);
            let ty = m.u16(at + 0x20).unwrap_or(0);
            let value = m.u32(cells + i * 8 + 4).unwrap_or(u32::MAX);
            let mut v = json!({ "bits": value });
            if ty == 6 {
                v["real"] = f64::from(f32::from_bits(value));
            } else if matches!(ty, 4 | 5 | 7 | 8) {
                v["number"] = value.into();
            }
            out.push(json!({
                "name": String::from_utf8_lossy(&b[..end]),
                "type": type_name(ty),
                "value": v,
            }));
        }
        Ok(())
    })?;
    Ok(json!({ "globals": out }))
}
