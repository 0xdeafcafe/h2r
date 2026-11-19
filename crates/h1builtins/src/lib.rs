//! Halo 1 builtins the recompiled game serves, and the table both sides
//! share (docs/halo1/scripts.md, "Serving missing builtins").
//!
//! Halo 2's engine lacks some of Halo 1's script builtins: no builtin of
//! that name its scripts call, and none at the position Halo 1's order
//! gives one ([`h1scripts::identified`] covers those that can be named by
//! position). The converter compiles a call of one of those to a call of a
//! **sacrificial slot**: a Halo 2 builtin index no retail script calls whose
//! retail evaluator is the shared discarded stub (`0x2ab130`, which returns
//! 0 without evaluating its arguments), keeping the Halo 1 name as the call
//! head and Halo 1's argument types on the argument nodes.
//!
//! The game's `h1-scripts` feature (crates/boot) then makes the call real:
//!
//! - it patches each slot's descriptor (in the guest's `.data`, once per
//!   run) to the served signature from this table and points its evaluator
//!   at the shared wrapper that evaluates arguments (`0x2ab6c0`, print's:
//!   generic over the descriptor, and reached only through the dispatch's
//!   `call [descriptor+4]`), and declares a passthrough return so the
//!   retail return path (`0x209ae0`) writes the value through uncast;
//! - it hooks the return path (`0x209ae0`, every builtin and script return
//!   flows through it) and, when the frame's call names a served slot,
//!   reads the arguments the wrapper evaluated into the frame, runs the
//!   Rust implementation registered for it, and puts its value in EAX
//!   before the retail body writes it out and pops the frame.
//!
//! With the feature off, or before a family registers an implementation,
//! the call runs the retail stub: the arguments are evaluated and the call
//! returns the served fallback ([`Served::done`]), the value every use in
//! the ten levels reads as done, so a `sleep_until` on it ends.
//!
//! A slot is claimed here only when it is one of the 288 Halo 2 builtins no
//! retail script calls whose evaluator is the shared stub (surveyed on the
//! disc; `h1campaign`'s disc test checks every entry). Entries are grouped
//! by family, in the order the lanes that own them landed them; take the
//! next unused slot from the survey, lowest first, and leave the note that
//! names the family. [`h1scripts::identified`] names other such slots by
//! position, so an identification and a served slot must not share one:
//! that disc test checks too.
//!
//! Type indices are Halo 2's (`hs::Catalog::TYPE_NAMES` in the retail XBE):
//! `passthrough` 3, `void` 4, `boolean` 5, `real` 6, `short` 7, `long` 8,
//! `string` 9. A signature here must be the Halo 2 mapping of Halo 1's own
//! signature for the builtin (the converter compiles argument nodes with
//! these types, and the wrapper's argument evaluator checks each node's
//! type against them).

/// One served Halo 1 builtin.
pub struct Served {
    /// The Halo 1 builtin's name, as the call head keeps it.
    pub name: &'static str,
    /// The Halo 2 builtin index the compiled call names (the sacrificial
    /// slot).
    pub slot: u16,
    /// Halo 2 return type. `PASSTHROUGH` unless Halo 1 returns void: the
    /// hook encodes the implementation's value for the call node's own
    /// type, and the retail return path writes a passthrough value through
    /// uncast, whatever the parent expects.
    pub returns: u16,
    /// Halo 2 parameter types, one per argument.
    pub params: &'static [u16],
    /// What the call returns while no implementation is registered: the
    /// value every use in the ten levels reads as done (0 for most).
    pub done: i32,
    /// Which lane's family owns the implementation.
    pub family: &'static str,
}

pub const PASSTHROUGH: u16 = 3;
pub const VOID: u16 = 4;
pub const BOOLEAN: u16 = 5;
pub const REAL: u16 = 6;
pub const SHORT: u16 = 7;
pub const LONG: u16 = 8;
pub const STRING: u16 = 9;

/// The shared discarded-stub evaluator every served slot's descriptor names
/// (`0x2ab130`: the thread from the stack, zero returned through the return
/// path `0x209ae0`, three arguments popped). The disc tests check each
/// claimed slot against it.
pub const NULL_STUB_EVALUATE: u32 = 0x002a_b130;

/// The served builtins. One per Halo 1 builtin; no two share a slot.
pub const SERVED: &[Served] = &[
    // The a30 scripts lane: the mission flow's own queries.
    // game_time (Halo 1 255): the ticks since the game started, which
    // a30's extraction timeouts read (`(< global_timer (game_time))`).
    // Halo 2's game globals keep it at `[[0x510c54]] + 8`.
    Served {
        name: "game_time",
        slot: 68,
        returns: PASSTHROUGH,
        params: &[],
        done: 0,
        family: "a30-scripts",
    },
];

/// The served builtin of `name`.
pub fn by_name(name: &str) -> Option<&'static Served> {
    SERVED.iter().find(|s| s.name == name)
}

/// The served builtin claimed for Halo 2 slot `slot`.
pub fn by_slot(slot: u16) -> Option<&'static Served> {
    SERVED.iter().find(|s| s.slot == slot)
}

/// Every Halo 2 builtin slot this table claims, with the family that owns
/// it. Lanes that hook builtin slots on converted maps claim them here (and
/// only here), so a merge can check for clashes; the AI-control lane's
/// slot 300, for one, is not among them.
pub fn claims() -> impl Iterator<Item = (&'static str, u16, &'static str)> {
    SERVED.iter().map(|s| (s.name, s.slot, s.family))
}

/// What a stubbed call returns for a status query the levels wait on: the
/// value every use in the ten levels reads as done, so a `sleep_until` on
/// it ends (docs/halo1/scripts.md, "Missing builtins"). The converter bakes
/// these into stubs; the serving feature returns them when a served builtin
/// has no implementation registered.
///
/// Served builtins carry their own [`Served::done`], which wins over an
/// entry here for them.
pub const DONE_VALUES: &[(&str, i32)] = &[
    ("ai_conversation_status", 5),
    ("ai_command_list_status", 2),
    ("game_all_quiet", 1),
    ("game_safe_to_speak", 1),
    // d40's escape timer: `(<= (hud_get_timer_ticks) 0)` ends the level
    // as failed; Halo 2 has no HUD timer, so it never runs out.
    ("hud_get_timer_ticks", 32767),
    // b30: `(< 1 (ai_conversation_line jeep_load))`, a line past the first.
    ("ai_conversation_line", 99),
];

/// What the stub of `name` returns, for the converter's stub policy: the
/// done value of the table above, or `None` for the type's zero. A served
/// builtin is never stubbed, so it has no stub value.
pub fn stub_value(name: &str) -> Option<i32> {
    if by_name(name).is_some() {
        return None;
    }
    DONE_VALUES.iter().find(|d| d.0 == name).map(|d| d.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No two served entries share a name or a slot, and the table stays in
    /// slot order (the survey's assignment order).
    #[test]
    fn served_do_not_collide() {
        for (i, a) in SERVED.iter().enumerate() {
            for b in &SERVED[i + 1..] {
                assert_ne!(a.name, b.name, "{a} and {b}", a = a.name, b = b.name);
                assert_ne!(a.slot, b.slot, "{} and {}", a.name, b.name);
            }
        }
        assert!(SERVED.windows(2).all(|w| w[0].slot < w[1].slot));
    }

    /// The first slots of the surveyed pool (unnamed, shared-stub evaluators
    /// in the retail Halo 2 XBE, ascending) — the numbers the disc test
    /// checks the table against, kept here so a reader can see where the
    /// next family's slot comes from without the discs.
    #[test]
    fn slots_come_from_the_surveyed_pool() {
        let pool: &[u16] = &[
            68, 116, 120, 121, 122, 130, 131, 132, 133, 134, 135, 147, 148, 149, 150, 151, 152,
            153, 154, 155, 156, 157, 158, 159,
        ];
        for s in SERVED {
            assert!(pool.contains(&s.slot), "{}: slot {} is not in the pool", s.name, s.slot);
        }
    }

    #[test]
    fn stub_values_skip_served() {
        assert_eq!(stub_value("game_time"), None);
        assert_eq!(stub_value("game_all_quiet"), Some(1));
        assert_eq!(stub_value("print"), None);
    }
}
