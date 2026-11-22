//! Reloading a running script: whether a new blob keeps the old one's
//! state shape, and carrying the slot values across when it does
//! (`docs/game/rules-engine.md`, "Reloading a running script").
//!
//! [`compatible`] is the whole decision. Two blobs have the same state
//! shape when a [`RulesExt`] of one is a valid state of the other and
//! means the same thing: the same carrier, team count, API version and
//! capabilities; the same options (name, type, range, default, in order),
//! so the record's option bits read the same; the same enums by name and
//! variant names; the same slots per scope (name and type, in order); the
//! same Forge labels; and the same palette names, which grants index.
//! What may change freely: handlers, functions, constant tables, messages,
//! tags, the script's name and version, option labels and descriptions,
//! and a slot's initial value (a running slot keeps its value; the new
//! initial value applies to rows the script has not written yet, such as a
//! new object's).
//!
//! [`carry`] then fixes the values that index a table of the blob: a
//! `Text` or `Class` slot holds a string-table index and a `Tag` slot a
//! tag-table index, and those tables may change. Each is moved to the
//! same string or tag in the new tables, or to the slot's new initial
//! value (`none` for a tag) when the new blob lacks it. Queued calls are
//! carried the same way. The VM's hook is [`crate::Vm::swap_program`].

use std::fmt;

use crate::ir::{Ir, Scope};
use crate::ty::{NONE, Ty};
use crate::vm::RulesExt;
use crate::{Refusal, caps};

/// One way the new blob's state shape differs from the old one's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The script rides another engine index.
    Carrier { old: u8, new: u8 },
    Teams { old: u8, new: u8 },
    ApiVersion { old: u16, new: u16 },
    /// The `requires` list differs (names or order).
    Capabilities,
    /// The option count differs.
    OptionCount { old: usize, new: usize },
    /// The option at this position differs in name, type, range or default.
    Option { name: String },
    EnumCount { old: usize, new: usize },
    /// The enum at this position differs in name or variants.
    Enum { name: String },
    /// A scope's slot count differs.
    SlotCount { scope: &'static str, old: usize, new: usize },
    /// The slot at this position of its scope differs in name or type.
    Slot { scope: &'static str, name: String },
    /// The Forge label table differs.
    Labels,
    /// The palette names differ (grants index them).
    Palettes,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::Carrier { old, new } => {
                write!(f, "the script rides engine {new}, not {old}")
            }
            Reason::Teams { old, new } => write!(f, "the script needs {new} teams, not {old}"),
            Reason::ApiVersion { old, new } => {
                write!(f, "the script uses API version {new}, not {old}")
            }
            Reason::Capabilities => f.write_str("the capabilities the script requires changed"),
            Reason::OptionCount { old, new } => {
                write!(f, "the script has {new} options, not {old}")
            }
            Reason::Option { name } => write!(f, "option {name} changed"),
            Reason::EnumCount { old, new } => write!(f, "the script has {new} enums, not {old}"),
            Reason::Enum { name } => write!(f, "enum {name} changed"),
            Reason::SlotCount { scope, old, new } => {
                write!(f, "the script has {new} {scope} variables, not {old}")
            }
            Reason::Slot { scope, name } => write!(f, "{scope} variable {name} changed"),
            Reason::Labels => f.write_str("the Forge labels the script names changed"),
            Reason::Palettes => f.write_str("the palettes the script declares changed"),
        }
    }
}

/// Why [`crate::Vm::swap_program`] kept the old program.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SwapError {
    #[error(transparent)]
    Refused(#[from] Refusal),
    #[error("the state shape changed: {}", reasons(.0))]
    Incompatible(Vec<Reason>),
}

/// The reasons as one line, `; ` between them.
pub fn reasons(r: &[Reason]) -> String {
    r.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
}

fn string(ir: &Ir, i: u16) -> &str {
    ir.strings.get(i as usize).map_or("?", String::as_str)
}

fn scope_name(s: Scope) -> &'static str {
    match s {
        Scope::Global => "global",
        Scope::Player => "player",
        Scope::Team => "team",
        Scope::Object => "object",
    }
}

/// The slots of one scope as (name, type), in slot order.
fn slots(ir: &Ir, scope: Scope) -> Vec<(&str, Ty)> {
    ir.slots
        .iter()
        .filter(|s| s.scope == scope)
        .map(|s| (string(ir, s.name), s.ty))
        .collect()
}

/// Whether `new` keeps `old`'s state shape: `Ok` when a running `old`
/// can take `new`'s program with every slot value kept, else every
/// difference found. Pure; see the module doc for what counts.
pub fn compatible(old: &Ir, new: &Ir) -> Result<(), Vec<Reason>> {
    let mut out = Vec::new();
    let (oh, nh) = (&old.header, &new.header);
    if oh.carrier != nh.carrier {
        out.push(Reason::Carrier { old: oh.carrier, new: nh.carrier });
    }
    if oh.teams != nh.teams {
        out.push(Reason::Teams { old: oh.teams, new: nh.teams });
    }
    if oh.api_version != nh.api_version {
        out.push(Reason::ApiVersion { old: oh.api_version, new: nh.api_version });
    }
    if oh.requires != nh.requires {
        out.push(Reason::Capabilities);
    }
    if old.options.len() != new.options.len() {
        out.push(Reason::OptionCount { old: old.options.len(), new: new.options.len() });
    } else {
        for (a, b) in old.options.iter().zip(&new.options) {
            let same = string(old, a.name) == string(new, b.name)
                && a.ty == b.ty
                && a.lo == b.lo
                && a.hi == b.hi
                && a.default == b.default;
            if !same {
                out.push(Reason::Option { name: string(new, b.name).to_string() });
            }
        }
    }
    if old.enums.len() != new.enums.len() {
        out.push(Reason::EnumCount { old: old.enums.len(), new: new.enums.len() });
    } else {
        for (a, b) in old.enums.iter().zip(&new.enums) {
            let names = |ir: &Ir, e: &crate::ir::EnumDef| -> Vec<String> {
                e.variants.iter().map(|&v| string(ir, v).to_string()).collect()
            };
            if string(old, a.name) != string(new, b.name) || names(old, a) != names(new, b) {
                out.push(Reason::Enum { name: string(new, b.name).to_string() });
            }
        }
    }
    for scope in [Scope::Global, Scope::Player, Scope::Team, Scope::Object] {
        let (a, b) = (slots(old, scope), slots(new, scope));
        if a.len() != b.len() {
            out.push(Reason::SlotCount { scope: scope_name(scope), old: a.len(), new: b.len() });
            continue;
        }
        for ((an, at), (bn, bt)) in a.iter().zip(&b) {
            if an != bn || at != bt {
                out.push(Reason::Slot { scope: scope_name(scope), name: (*bn).to_string() });
            }
        }
    }
    if old.labels != new.labels {
        out.push(Reason::Labels);
    }
    let palettes = |ir: &Ir| -> Vec<String> {
        ir.palettes.iter().map(|p| string(ir, p.name).to_string()).collect()
    };
    if palettes(old) != palettes(new) {
        out.push(Reason::Palettes);
    }
    if out.is_empty() { Ok(()) } else { Err(out) }
}

/// A raw value of type `ty` from `old`'s tables, as `new` reads it;
/// `fallback` when `new` has no such string or tag.
fn moved(old: &Ir, new: &Ir, ty: Ty, v: i32, fallback: i32) -> i32 {
    let find = |from: &[String], to: &[String]| -> i32 {
        let Some(s) = usize::try_from(v).ok().and_then(|i| from.get(i)) else {
            return v;
        };
        to.iter()
            .position(|t| t == s)
            .and_then(|i| i32::try_from(i).ok())
            .unwrap_or(fallback)
    };
    match ty {
        Ty::Text | Ty::Class => find(&old.strings, &new.strings),
        Ty::Tag => {
            let paths = |ir: &Ir| -> Vec<String> { ir.tags.iter().map(|t| t.path.clone()).collect() };
            find(&paths(old), &paths(new))
        }
        _ => v,
    }
}

/// Moves the values in `ext` that index `old`'s string and tag tables to
/// `new`'s (module doc). Call only when [`compatible`] said yes: the slot
/// layout is then the same.
pub fn carry(old: &Ir, new: &Ir, ext: &mut RulesExt) {
    let mut at = [0usize; 4];
    for s in &new.slots {
        let scope = s.scope as usize;
        let i = at[scope];
        at[scope] += 1;
        if !matches!(s.ty, Ty::Text | Ty::Class | Ty::Tag) {
            continue;
        }
        let fallback = if s.ty == Ty::Tag { NONE } else { s.init };
        let mut fix = |row: &mut Vec<i32>| {
            if let Some(v) = row.get_mut(i) {
                *v = moved(old, new, s.ty, *v, fallback);
            }
        };
        match s.scope {
            Scope::Global => fix(&mut ext.globals),
            Scope::Player => ext.players.iter_mut().for_each(&mut fix),
            Scope::Team => ext.teams.iter_mut().for_each(&mut fix),
            Scope::Object => ext.objects.values_mut().for_each(&mut fix),
        }
    }
    let api = new.header.api_version;
    for q in &mut ext.queue {
        let Some(entry) = caps::entry(q.entry, api) else {
            continue;
        };
        for (&ty, v) in entry.args.iter().zip(q.args.iter_mut()) {
            let fallback = if ty == Ty::Text { 0 } else { NONE };
            *v = moved(old, new, ty, *v, fallback);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeGame;
    use crate::vm::{Input, Vm};
    use crate::{Event, compile};

    const BASE: &str = r#"
rules "Count" 1
carrier slayer

option kill_points: int 1..5 = 1 "Kill points" "Points for each kill"

global var kills: int = 0
player var streak: int = 0

on kill(victim, killer) {
    if killer != none and killer != victim {
        kills = kills + 1
        killer.streak = killer.streak + 1
        score(killer, kill_points)
    }
}
"#;

    fn ir(src: &str) -> Ir {
        compile(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    fn edited(from: &str, to: &str) -> Ir {
        let src = BASE.replacen(from, to, 1);
        assert_ne!(src, BASE, "the edit {from:?} did not apply");
        ir(&src)
    }

    #[test]
    fn the_same_script_is_compatible() {
        assert_eq!(compatible(&ir(BASE), &ir(BASE)), Ok(()));
    }

    #[test]
    fn a_handler_change_is_compatible() {
        let new = edited("score(killer, kill_points)", "score(killer, kill_points + 1)");
        assert_eq!(compatible(&ir(BASE), &new), Ok(()));
        let tick = BASE.to_string() + "\non tick {\n    kills = kills\n}\n";
        assert_eq!(compatible(&ir(BASE), &ir(&tick)), Ok(()));
    }

    #[test]
    fn a_changed_initial_value_is_compatible() {
        let new = edited("global var kills: int = 0", "global var kills: int = 7");
        assert_eq!(compatible(&ir(BASE), &new), Ok(()));
    }

    #[test]
    fn a_changed_option_label_is_compatible() {
        let new = edited("\"Kill points\"", "\"Points a kill\"");
        assert_eq!(compatible(&ir(BASE), &new), Ok(()));
    }

    #[test]
    fn an_added_variable_is_not() {
        let new = edited(
            "player var streak: int = 0",
            "player var streak: int = 0\nplayer var best: int = 0",
        );
        assert_eq!(
            compatible(&ir(BASE), &new),
            Err(vec![Reason::SlotCount { scope: "player", old: 1, new: 2 }])
        );
    }

    #[test]
    fn a_changed_type_is_not() {
        let src = BASE
            .replacen("global var kills: int = 0", "global var kills: bool = false", 1)
            .replacen("kills = kills + 1", "kills = true", 1);
        assert_eq!(
            compatible(&ir(BASE), &ir(&src)),
            Err(vec![Reason::Slot { scope: "global", name: "kills".into() }])
        );
    }

    #[test]
    fn a_changed_option_is_not() {
        let range = edited("int 1..5 = 1", "int 1..9 = 1");
        assert_eq!(
            compatible(&ir(BASE), &range),
            Err(vec![Reason::Option { name: "kill_points".into() }])
        );
        let default = edited("int 1..5 = 1", "int 1..5 = 2");
        assert_eq!(
            compatible(&ir(BASE), &default),
            Err(vec![Reason::Option { name: "kill_points".into() }])
        );
        let added = edited(
            "option kill_points",
            "option suicide_costs: bool = true\noption kill_points",
        );
        assert_eq!(
            compatible(&ir(BASE), &added),
            Err(vec![Reason::OptionCount { old: 1, new: 2 }])
        );
    }

    #[test]
    fn reordered_variables_are_not() {
        let src = BASE.replacen(
            "global var kills: int = 0",
            "global var deaths: int = 0\nglobal var kills: int = 0",
            1,
        );
        let two = ir(&src);
        let swapped = ir(&BASE.replacen(
            "global var kills: int = 0",
            "global var kills: int = 0\nglobal var deaths: int = 0",
            1,
        ));
        assert_eq!(
            compatible(&two, &swapped),
            Err(vec![
                Reason::Slot { scope: "global", name: "kills".into() },
                Reason::Slot { scope: "global", name: "deaths".into() },
            ])
        );
    }

    #[test]
    fn another_carrier_is_not() {
        let new = edited("carrier slayer", "carrier oddball");
        let why = compatible(&ir(BASE), &new).unwrap_err();
        assert!(matches!(why[0], Reason::Carrier { .. }), "{why:?}");
        assert!(reasons(&why).contains("rides engine"), "{}", reasons(&why));
    }

    fn kill(vm: &mut Vm, g: &mut FakeGame, killer: u8, victim: u8) {
        vm.fire(g, Input::Kill { victim, killer: Some(killer) });
    }

    #[test]
    fn an_in_place_swap_keeps_slot_values_and_runs_the_new_handlers() {
        let mut g = FakeGame::with_players(2);
        let mut vm = Vm::new(ir(BASE), 1, 0).unwrap();
        vm.begin_tick(1);
        kill(&mut vm, &mut g, 0, 1);
        kill(&mut vm, &mut g, 0, 1);
        assert_eq!(vm.global("kills"), Some(2));
        assert_eq!(vm.player_var(0, "streak"), Some(2));
        let scored = g.players[&0].score;

        // A new handler body, a new initial value and a new tick handler.
        let src = BASE
            .replacen("global var kills: int = 0", "global var kills: int = 50", 1)
            .replacen("score(killer, kill_points)", "score(killer, kill_points * 10)", 1);
        let new = ir(&src);
        let hash = new.id();
        vm.swap_program(new).unwrap();
        assert_eq!(vm.ir().id(), hash);
        assert_eq!(vm.global("kills"), Some(2), "the value is kept, not the new initial value");
        assert_eq!(vm.player_var(0, "streak"), Some(2));

        vm.begin_tick(2);
        kill(&mut vm, &mut g, 0, 1);
        assert_eq!(vm.global("kills"), Some(3));
        // Two kills scored `scored`, so one is worth `scored / 2`; the new
        // handler gives ten times that.
        assert_eq!(g.players[&0].score, scored + scored / 2 * 10, "the new handler ran");
    }

    #[test]
    fn a_swap_with_another_shape_keeps_the_old_program() {
        let mut vm = Vm::new(ir(BASE), 1, 0).unwrap();
        let old = vm.ir().id();
        let new = edited(
            "player var streak: int = 0",
            "player var streak: int = 0\nplayer var best: int = 0",
        );
        match vm.swap_program(new) {
            Err(SwapError::Incompatible(r)) => {
                assert_eq!(r, vec![Reason::SlotCount { scope: "player", old: 1, new: 2 }])
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(vm.ir().id(), old);
    }

    #[test]
    fn a_swap_clears_a_stopped_script() {
        let mut vm = Vm::new(ir(BASE), 1, 0).unwrap();
        let mut ext = vm.ext().clone();
        ext.stopped = Some(crate::ScriptError {
            tick: 1,
            handler: 0,
            pc: 0,
            kind: crate::vm::ErrorKind::Budget,
        });
        let mut vm = Vm::resume(vm.ir().clone(), 1, ext).unwrap();
        assert!(vm.ext().stopped.is_some());
        vm.swap_program(ir(BASE)).unwrap();
        assert!(vm.ext().stopped.is_none());
        assert!(vm.ir().handlers.iter().any(|h| h.event == Event::Kill));
    }

    /// A `text` slot holds a string-table index: it follows its string
    /// into the new table, or takes its initial value when the string is
    /// gone. (Built by hand: only the tables matter to `carry`.)
    #[test]
    fn text_values_follow_their_string_into_the_new_table() {
        use crate::ir::Slot;
        let mut old = ir(BASE);
        let intern = |ir: &mut Ir, s: &str| -> u16 {
            ir.strings.push(s.to_string());
            (ir.strings.len() - 1) as u16
        };
        let name = intern(&mut old, "said");
        let hello = intern(&mut old, "hello");
        let bye = intern(&mut old, "bye");
        old.slots.push(Slot { name, scope: Scope::Global, ty: Ty::Text, init: hello as i32 });
        let mut new = old.clone();
        // "bye" moves to the end of the table; "hello" stays where it was.
        new.strings.remove(bye as usize);
        new.strings.push("other".into());
        new.strings.push("bye".into());
        assert_eq!(compatible(&old, &new), Ok(()));
        let vm = Vm::new(old.clone(), 1, 0).unwrap();
        let mut ext = vm.ext().clone();
        let said = ext.globals.len() - 1;
        ext.globals[said] = bye as i32;
        carry(&old, &new, &mut ext);
        assert_eq!(new.strings[ext.globals[said] as usize], "bye");

        // A string the new table lacks: the slot's initial value.
        let mut gone = old.clone();
        gone.strings[bye as usize] = "farewell".into();
        let mut ext = vm.ext().clone();
        ext.globals[said] = bye as i32;
        carry(&old, &gone, &mut ext);
        assert_eq!(ext.globals[said], hello as i32);
    }
}
