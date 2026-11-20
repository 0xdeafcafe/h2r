//! The retail interpreter's value casts, and a check that a compiled
//! scenario only asks for casts the interpreter has.
//!
//! **E.** Every evaluated value is converted to its node's type by
//! `0x0020a290(value, to = cx, from = dx)`: literal atoms from their source
//! type (node `+2`, `0x00209a88`), globals from the global's type
//! (`0x00209a33`), calls from the builtin's or script's return type on frame
//! return (`0x00209ae0`). The converter returns the value unchanged when
//! `from == to`, `from` is passthrough, `to` is a name type
//! (`object_name`..`scenery_name`), or `to` is an object type
//! (`object`..`scenery`) and `from` is not a name; a name to an object type
//! goes through `0x000bb760`; otherwise it calls
//! `table[to * 62 + from]` at `0x004f5770` **without a null check**. The table is
//! all null but for what `0x0020a1a0` writes at startup ([`castable`]). A
//! pair it lacks calls address 0 (b30's first fault: a Halo 1 string literal
//! where Halo 2's `vehicle_load_magic` wants a `unit_seat_mapping`).
use crate::{Catalog, Result, Scenario};

/// Type indices the casts name (Halo 2's type table, `0x0046ff00`).
pub mod types {
    pub const PASSTHROUGH: u16 = 3;
    pub const VOID: u16 = 4;
    pub const BOOLEAN: u16 = 5;
    pub const REAL: u16 = 6;
    pub const SHORT: u16 = 7;
    pub const LONG: u16 = 8;
    pub const STRING: u16 = 9;
    pub const AI: u16 = 19;
    pub const OBJECT_LIST: u16 = 31;
    pub const GAME_DIFFICULTY: u16 = 44;
    pub const NETWORK_EVENT: u16 = 49;
    pub const OBJECT: u16 = 50;
    pub const SCENERY: u16 = 55;
    pub const OBJECT_NAME: u16 = 56;
    pub const SCENERY_NAME: u16 = 61;
    /// Types in the table (`62`).
    pub const COUNT: u16 = 62;
}

/// Whether the retail interpreter can convert a `from` value to `to`.
pub fn castable(to: u16, from: u16) -> bool {
    use types::*;
    let objects = OBJECT..=SCENERY;
    let names = OBJECT_NAME..=SCENERY_NAME;
    if from == to || from == PASSTHROUGH || names.contains(&to) {
        return true;
    }
    // An object type takes any value: a name through `0x000bb760`, anything
    // else unchanged (`0x0020a2a7`..`0x0020a2bf`).
    if objects.contains(&to) {
        return true;
    }
    match to {
        VOID => (BOOLEAN..COUNT).contains(&from),
        BOOLEAN => matches!(from, REAL | SHORT | LONG | STRING),
        REAL => matches!(from, SHORT | LONG) || (GAME_DIFFICULTY..=NETWORK_EVENT).contains(&from),
        SHORT => matches!(from, REAL | LONG),
        LONG => matches!(from, REAL | SHORT),
        OBJECT_LIST => from == AI || objects.contains(&from) || names.contains(&from),
        _ => false,
    }
}

/// A node whose value the retail interpreter cannot convert, or a call it
/// cannot dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unrunnable {
    /// Node slot.
    pub slot: usize,
    /// What is wrong, with type and builtin names.
    pub what: String,
}

impl Scenario {
    /// Every node reached from a script or global root that the retail
    /// interpreter could not evaluate: a cast [`castable`] lacks, a builtin
    /// index past the catalog, or a builtin with no evaluator.
    pub fn unrunnable(&self, catalog: &Catalog) -> Result<Vec<Unrunnable>> {
        let name = |t: u16| catalog.type_name(t).unwrap_or("?").to_owned();
        let mut out = Vec::new();
        for d in self.reachable(self.roots())? {
            let n = self.node(d)?;
            let slot = d.slot();
            let from = if n.atom() {
                if n.value_type == 2 {
                    continue; // a call's function-name head
                }
                if n.global() {
                    let index = crate::expression::global_word(n.data)?;
                    if index & 0x8000 != 0 {
                        match catalog.external_global_types.get((index & 0x7fff) as usize) {
                            Some(&t) => t,
                            None => continue, // validate() reports it
                        }
                    } else {
                        match self.globals.get(index as usize) {
                            Some(g) => g.value_type,
                            None => continue,
                        }
                    }
                } else {
                    n.opcode
                }
            } else if n.script_call() {
                match self.scripts.get(n.opcode as usize) {
                    Some(s) => s.return_type,
                    None => continue,
                }
            } else {
                match catalog.functions().get(n.opcode as usize) {
                    None => {
                        out.push(Unrunnable {
                            slot,
                            what: format!("builtin {:#x} is past the catalog", n.opcode),
                        });
                        continue;
                    }
                    Some(f) if f.evaluate() == 0 => {
                        out.push(Unrunnable {
                            slot,
                            what: format!("builtin {} has no evaluator", f.name().unwrap_or("?")),
                        });
                        continue;
                    }
                    Some(f) => f.return_type(),
                }
            };
            if !castable(n.value_type, from) {
                out.push(Unrunnable {
                    slot,
                    what: format!(
                        "{} {} to {}: no cast",
                        if n.atom() { "atom" } else { "call" },
                        name(from),
                        name(n.value_type)
                    ),
                });
            }
        }
        Ok(out)
    }
}
