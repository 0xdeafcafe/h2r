//! What the language's names mean (the docs hover and completion show),
//! and the features that read the model directly: diagnostics, hover,
//! signature help, definitions, references, highlights, semantic tokens
//! and the outline.

use super::model::{DefKind, Model, Ref, T, Use, is_type_word};
use super::scan::K;
use super::{
    Diagnostic, Highlight, Hover, Location, Markup, Parameter, Position, Range, Signature, SignatureHelp, Symbol, Token,
    TokenKind, highlight_kind, severity, symbol_kind,
};
use crate::caps::{self, Entry, Kind, Order, Status, VARIANT_FIELDS};
use crate::ir::{self, Event, LabelKind, Scope};
use crate::lang::ast::Item;
use crate::lang::lower::FIELDS;
use crate::lang::parse::KEYWORDS;
use crate::lang::sig;
use crate::lang::types::Type;
use crate::lang::{MAX_SOURCE_BYTES, Pos, Severity};
use crate::settings::{self, Kind as SettingKind};
use crate::ty::Ty;

/// A language built-in the compiler answers itself.
pub(crate) struct Lang {
    pub name: &'static str,
    pub params: &'static [&'static str],
    /// Empty for none.
    pub result: &'static str,
    pub doc: &'static str,
    pub snippet: &'static str,
}

pub(crate) const LANG: &[Lang] = &[
    Lang {
        name: "count",
        params: &["set"],
        result: "int",
        doc: "How many members a set has: `count(players p where p.alive)`. The set may name its member for the `where`.",
        snippet: "count(${1:players})",
    },
    Lang {
        name: "first",
        params: &["set"],
        result: "the member type",
        doc: "A set's first member (players by slot, teams by index, objects by placement), or `none`: `first(players p where p.team == t)`.",
        snippet: "first(${1:players} ${2:p} where ${3:p.alive})",
    },
    Lang {
        name: "pick",
        params: &["set", "n: int"],
        result: "",
        doc: "`n` members of a set, drawn by the match's generator: a set for `for each` and `count`. The set has no name and no `where`.",
        snippet: "pick(${1:players}, ${2:1})",
    },
    Lang {
        name: "len",
        params: &["table"],
        result: "int",
        doc: "A table's row count.",
        snippet: "len(${1:table})",
    },
    Lang {
        name: "seconds",
        params: &["n: int"],
        result: "time",
        doc: "`n` seconds as a time (30 ticks a second), for an int worked out at run time; a literal is written `5s`.",
        snippet: "seconds(${1:n})",
    },
    Lang {
        name: "whole_seconds",
        params: &["t: time"],
        result: "int",
        doc: "The whole seconds in a time, rounded down: for a score or a message.",
        snippet: "whole_seconds(${1:t})",
    },
    Lang {
        name: "ticks",
        params: &["t: time"],
        result: "int",
        doc: "A time's tick count (30 a second), as an int.",
        snippet: "ticks(${1:t})",
    },
    Lang {
        name: "grant",
        params: &["p: player", "capability", "budget n", "palette \"x\""],
        result: "",
        doc: "Gives a player a Forge power: a grantable capability the script requires, a budget of uses and, optionally, a palette of tags. It ends at `revoke`, the player's quit or the round's end.",
        snippet: "grant(${1:p}, ${2:forge.place}, budget ${3:10})",
    },
    Lang {
        name: "revoke",
        params: &["p: player", "capability"],
        result: "",
        doc: "Ends a player's grant of a capability.",
        snippet: "revoke(${1:p}, ${2:forge.place})",
    },
    Lang {
        name: "granted",
        params: &["p: player", "capability"],
        result: "bool",
        doc: "Whether a player holds a grant of a capability.",
        snippet: "granted(${1:p}, ${2:forge.place})",
    },
];

pub(crate) fn lang_index(name: &str) -> Option<usize> {
    LANG.iter().position(|l| l.name == name)
}

const NO_ARGS: &[&str] = &[];
const KILL: &[&str] = &["victim", "killer"];
const P: &[&str] = &["p"];
const P_O: &[&str] = &["p", "o"];
const ZONE_O: &[&str] = &["zone", "o"];
const T_ARG: &[&str] = &["t"];
const S: &[&str] = &["s"];
const KILLER_CLASS: &[&str] = &["killer", "class"];
const BOUNCE: &[&str] = &["p", "spot", "chain", "monster"];
const KILLER_ASSIST_CLASS: &[&str] = &["killer", "assist", "class"];

/// An event's argument names, as the docs write them, and what fires it.
pub(crate) fn event_info(ev: Event) -> (&'static [&'static str], &'static str) {
    match ev {
        Event::Init => (NO_ARGS, "Once, when the script starts, before the first round."),
        Event::RoundStart => (NO_ARGS, "At the start of each round."),
        Event::Tick => (NO_ARGS, "Every tick (30 a second), after the timers."),
        Event::Kill => (
            KILL,
            "A player died. `killer` is `none` for a death to the world, and the victim for a suicide.",
        ),
        Event::Spawn => (P, "A player spawned."),
        Event::Death => (P, "A player died."),
        Event::Pickup => (P_O, "A player picked up an object: a weapon, a flag or a ball."),
        Event::Drop => (P_O, "A player dropped an object."),
        Event::Enter => (ZONE_O, "An object entered a zone."),
        Event::Leave => (ZONE_O, "An object left a zone."),
        Event::Press => (P_O, "A player pressed a device; at most 10 a second per player."),
        Event::Timer => (
            T_ARG,
            "A global `timer` reached 0. The handler names the timer: `on timer(t)`.",
        ),
        Event::SquadDead => (S, "A squad's last actor died."),
        Event::ActorKill => (KILLER_CLASS, "A player killed an AI actor of a kit class."),
        Event::ActorAssist => (
            KILLER_ASSIST_CLASS,
            "A player assisted another's kill of an AI actor of a kit class.",
        ),
        Event::Bounce => (
            BOUNCE,
            "A player superbounced off a strip. `chain` is the bounce's place in its chain (1 for a single bounce, 2 for a double); `monster` is true for a Monster Bounce, a launch well above a normal one.",
        ),
        Event::Edit => (P_O, "A player used a granted power."),
        Event::Join => (P, "A player joined."),
        Event::Quit => (P, "A player quit."),
        Event::RoundEnd => (NO_ARGS, "At the end of each round."),
        Event::Bounced => (P, "A player fell out of the map during a bounce: the Bounced medal."),
    }
}

/// `kill(victim: player, killer: player?)`, in the event's front-end
/// types (`lang::sig::event_args`), and each argument's offsets.
pub(crate) fn event_signature(ev: Event) -> (String, Vec<[u32; 2]>) {
    let (names, _) = event_info(ev);
    let tys: Vec<String> = if ev == Event::Timer {
        vec!["timer".to_string()]
    } else {
        sig::event_args(ev).iter().map(|t| t.name(&[])).collect()
    };
    let params: Vec<String> = names
        .iter()
        .zip(tys)
        .map(|(n, t)| format!("{n}: {t}"))
        .collect();
    call_label(ev.name(), &params, "")
}

/// `name(a, b) -> r`, with each parameter's offsets in the label.
fn call_label(name: &str, params: &[String], result: &str) -> (String, Vec<[u32; 2]>) {
    let mut label = format!("{name}(");
    let mut offsets = Vec::with_capacity(params.len());
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let s = label.encode_utf16().count() as u32;
        label.push_str(p);
        offsets.push([s, label.encode_utf16().count() as u32]);
    }
    label.push(')');
    if !result.is_empty() {
        label.push_str(" -> ");
        label.push_str(result);
    }
    (label, offsets)
}

/// An entry's parameters and result in its front-end signature
/// (`lang::sig::of`): `inside(player, zone) -> bool`. `message` keeps its
/// three-form spelling.
pub(crate) fn entry_parts(e: &Entry) -> (Vec<String>, String) {
    let message = matches!(e.id, caps::id::MESSAGE | caps::id::MESSAGE_TEAM | caps::id::MESSAGE_ALL);
    match sig::of(e) {
        Some(s) if !message => (
            s.args.iter().map(|t| t.name(&[])).collect(),
            s.result.map_or_else(String::new, |t| t.name(&[])),
        ),
        _ => (caps::params(e), e.result.map_or("", Ty::name).to_string()),
    }
}

pub(crate) fn entry_signature(e: &Entry) -> (String, Vec<[u32; 2]>) {
    let (params, result) = entry_parts(e);
    call_label(caps::spelled(e), &params, &result)
}

pub(crate) fn lang_signature(l: &Lang) -> (String, Vec<[u32; 2]>) {
    let params: Vec<String> = l.params.iter().map(|p| p.to_string()).collect();
    call_label(l.name, &params, l.result)
}

/// A user fn's signature from its parameters.
pub(crate) fn fn_signature(m: &Model, d: usize) -> (String, Vec<[u32; 2]>) {
    let params: Vec<String> = m
        .children(d, DefKind::Param)
        .map(|p| format!("{}: {}", m.defs[p].name, m.type_name(m.defs[p].ty)))
        .collect();
    // A value function (`fn f(..) -> bool = ...`) has a result.
    let result = match m.defs[d].ty {
        T::Unknown => String::new(),
        t => m.type_name(t),
    };
    call_label(&m.defs[d].name, &params, &result)
}

/// A time literal's hover: what it is in ticks.
pub(crate) fn time_literal_doc(text: &str) -> String {
    let (number, unit) = text.split_at(text.len() - 1);
    let (whole, frac) = number.split_once('.').unwrap_or((number, ""));
    let per: u64 = if unit == "s" { caps::TICKS_PER_SECOND as u64 } else { 1 };
    let scale = 10u64.saturating_pow(frac.len() as u32);
    let value = whole
        .parse::<u64>()
        .ok()
        .zip(if frac.is_empty() { Some(0) } else { frac.parse::<u64>().ok() })
        .map(|(w, f)| (w.saturating_mul(scale).saturating_add(f)).saturating_mul(per));
    match value {
        Some(v) if v % scale == 0 => format!("A time literal: {} ticks (30 a second).", v / scale),
        Some(_) => "A time literal that is not a whole number of ticks (30 a second).".to_string(),
        None => "A time literal.".to_string(),
    }
}

pub(crate) fn status_words(s: Status) -> &'static str {
    match s {
        Status::Named => "named. A host makes this call.",
        Status::Traced => "traced: the entry's address is in a doc, with no named point yet. No host can make this call yet.",
        Status::Untraced => "untraced: there is no known engine entry. No host can make this call yet.",
        Status::NotBuilt => "not built: its game-side part (labels, zones or the detector) is not built. No host can make this call yet.",
        Status::VmOnly => "answered by the VM.",
    }
}

fn status_short(s: Status) -> &'static str {
    match s {
        Status::Named => "named",
        Status::Traced => "traced, not named",
        Status::Untraced => "untraced",
        Status::NotBuilt => "not built",
        Status::VmOnly => "the VM's",
    }
}

/// An API entry's documentation: what it is, its capability in the
/// lobby's words, and its status.
pub(crate) fn entry_doc(e: &Entry) -> String {
    let what = match e.kind {
        Kind::Read => "A read of the game.",
        Kind::Action => "An action.",
        Kind::VmOnly => "A VM built-in.",
    };
    let order = if e.order == Order::Queued {
        " Queued: made in program order at the next tick boundary."
    } else {
        ""
    };
    let cap = match e.cap {
        Some(c) => {
            let words = caps::capability(c, caps::API_VERSION).map_or("", |c| c.words);
            format!("Needs `requires {c}`: {words}.")
        }
        None => "Core: no capability to declare.".to_string(),
    };
    format!("{what}{order}\n\n{cap}\n\nStatus: {}", status_words(e.status))
}

/// What a type word means: the front-end types of
/// docs/game/rules-language-types.md, "Front-end types", and `timer`.
pub(crate) fn type_word_doc(w: &str) -> Option<&'static str> {
    Some(match w {
        "int" => "A 32-bit integer; arithmetic saturates.",
        "fixed" => "A 16.16 fixed-point number, for seconds and distances.",
        "bool" => "`true` or `false`.",
        "time" => "A duration or a tick count, in ticks (30 a second).",
        "player" => "A player (a roster slot). `player?` may be `none`.",
        "team" => "A team (0 to 7). `team?` may be `none`.",
        "object" => "Anything placed or made that has a handle. `object?` may be `none`.",
        "zone" => "An object with a volume: a Forge trigger volume, or a netgame flag with its capture volume. A zone is an object.",
        "squad" => "A kit squad slot, 0 to 15: `squad(n)`.",
        "timer" => "A global variable that counts ticks down; 0 is stopped. Reaching 0 fires `on timer`.",
        "Class" | "class" => "A kit class name.",
        "tag" => "A tag path of any object definition, checked against the map at load.",
        "weapon" => "A tag path that must be a weapon's. A weapon is a tag.",
        "text" => "A message: `text \"...\"`.",
        "objects" => "A set of objects: a label's kind, or a set parameter.",
        "zones" => "A set of zones: a label's kind, or a set parameter. A set of zones is a set of objects.",
        "spawns" => "Spawn areas: a label's kind, passed to `spawn_areas`, never visited.",
        _ => return None,
    })
}

pub(crate) fn keyword_doc(w: &str) -> Option<&'static str> {
    Some(match w {
        "rules" => "The header: `rules \"Name\" version`.",
        "carrier" => "Gone: a script names its game type with `type`, not an engine: `type slayer`.",
        "const" => "A named constant, folded at compile time and usable wherever a literal is: `const last_phase = 2`, `const hold = 30s`.",
        "match" => "One arm per variant of an enum value: `match role { Role.vip { } Role.guard, Role.none { } }`, or an `else` arm. Every variant needs an arm.",
        "teams" => "In the header, the team count (1 to 8): `teams 2`. In a set, every team.",
        "list" => "Gone: `type` files a variant in its row: `type research`, `type party`.",
        "type" => "The game type: `type slayer`. It picks the game type list's row the script's variants are filed under, the announcer's line at the start and what a point is called. A type changes no rule.",
        "points" => "What a point is, for the scoreboard and the post-game: `points \"Captures\"`. With no `points` line it is the type's.",
        "uses" => "The built-in game whose rules the script plays under its own settings: `uses slayer`. The script then holds its header lines, `settings` and `quick` only.",
        "settings" => "The record settings the script sets, one `name = value` a line: `settings { score_limit = 50; shields = none }`. A setting the block leaves out has the record's stock value.",
        "quick" => "The settings and options the lobby's Quick Options shows, at most five, in row order: `quick score_limit, starting_weapon`.",
        "scoring" => "Gone: `uses slayer` plays a built-in game's rules under the script's settings.",
        "about" => "The variant's description, at most 150 characters: `about \"No shields. 50 points to win.\"`.",
        "requires" => "The capabilities the script declares: `requires player.weapons, hud.message`. The lobby shows them in plain words.",
        "option" => "A row of the variant editor: `option name: int 1..5 = 1 \"Label\" \"Description\"`. An int, a bool or an enum, read-only in handlers.",
        "palette" => "A named list of tag paths: `palette \"x\" = [\"objects/...\"]`. `palette \"x\"` is a `grant` argument.",
        "enum" => "A script-declared type: `enum Role { none, vip }`, read as `Role.vip`.",
        "global" | "var" => "A variable: `global|player|team|object var name: type = init`.",
        "player" => "A player, or `none`. `player var` declares a variable of each player.",
        "team" => "A team, or `none`; `team 0` is a team value. `team var` declares a variable of each team.",
        "object" => "An object handle, or `none` as `object?`. `object var` declares a variable of each object.",
        "zone" => "An object with a volume: a Forge trigger volume, or a netgame flag. A zone is an object.",
        "weapon" => "A tag path that must be a weapon's. A weapon is a tag.",
        "table" => "Constant rows of typed columns: `table t: (k: int, v: int) = [(0, 1)]`; `t[i].v` reads a cell.",
        "fn" => "A function, inlined at each call; it does not recurse. A block function has no value; a value function gives one expression: `fn lead(t: team) -> bool = t.score > 10`.",
        "on" => "A handler: `on kill(victim, killer) { ... }`. It runs to completion.",
        "if" | "else" => "`if cond { } else if cond { } else { }`.",
        "for" | "each" => "A loop over a finite set: `for each players p where p.alive { }`.",
        "where" => "A set's filter. It may read the member's fields without naming it.",
        "let" => "A name for a value, not assigned again: `let n = count(players)`.",
        "and" | "or" => "Boolean operators; both short-circuit, and `and` binds tighter than `or`.",
        "not" => "Boolean negation.",
        "true" | "false" => "A `bool`.",
        "none" => "No player, team, object or tag.",
        "label" => "A Forge label the map supplies: `label goal: zone`, `label barrier: objects numbered`.",
        "is" => "Whether an object is a member of a label: `o is goal`.",
        "text" => "A message: `text \"...\"`, or a string-table index.",
        "budget" => "A grant's uses: `budget 20`.",
        "players" => "The set of players, by slot.",
        "objects" | "zones" | "spawns" => "A label's kind: `label barrier: objects`. A set of placements is a declared label.",
        "objectives" => "The objective registry's objects, at most 8, by handle.",
        "variant" => "The retail variant record: `variant.<field>` reads a field.",
        "numbered" => "A label's requirement: each member has a number (`numbered`), or one of each number in a range (`numbered 0..1`). `capture[n]` reads the member numbered `n`.",
        "per" => "A label's requirement: `per team`, members for each team; `base[t]` reads a team's.",
        "at" | "least" | "most" | "exactly" => "A label's count: `at least 2`, `at most 16`, `exactly 3`, or `2..4`.",
        _ => return None,
    })
}

/// A word the semantic tokens call a keyword when nothing else claims it:
/// the grammar's keywords, the set words and a label's requirement words.
pub(crate) fn is_keyword(w: &str) -> bool {
    KEYWORDS.contains(&w)
        || matches!(
            w,
            "player" | "object" | "players" | "objects" | "objectives" | "variant" | "numbered" | "per" | "at" | "least"
                | "most" | "exactly"
        )
}

const OPERATORS: &[&str] = &["==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "=", ".."];

fn code_block(text: &str) -> String {
    format!("```rules\n{text}\n```")
}

/// A declaration's hover text.
pub(crate) fn def_markdown(m: &Model, d: usize) -> String {
    let def = &m.defs[d];
    let name = &def.name;
    let ty = m.type_name(def.ty);
    let decl = || code_block(m.line_at(def.span.0).trim_end_matches('{').trim_end());
    match def.kind {
        DefKind::Option => format!(
            "{}\nAn option: a row of the variant editor, read-only in handlers.",
            decl()
        ),
        DefKind::Var(scope) => {
            let what = match scope {
                Scope::Global => "A global variable.".to_string(),
                Scope::Player => format!("A variable of each player, read as `p.{name}`."),
                Scope::Team => format!("A variable of each team, read as `t.{name}`."),
                Scope::Object => format!("A variable of each labelled object, read as `o.{name}`."),
            };
            format!("{}\n{what}", decl())
        }
        DefKind::Palette => format!("{}\nA palette: the tags a grant may place.", decl()),
        DefKind::Enum => {
            let n = m.children(d, DefKind::Member).count();
            format!("{}\nAn enum of {n} variants.", decl())
        }
        DefKind::Member => {
            let e = def.parent.map_or("?", |p| m.defs[p].name.as_str());
            format!("{}\nA variant of enum `{e}`.", code_block(&format!("{e}.{name}")))
        }
        DefKind::Table => format!("{}\nA table of constant rows.", decl()),
        DefKind::Column => {
            let t = def.parent.map_or("?", |p| m.defs[p].name.as_str());
            format!(
                "{}\nA column of table `{t}`: `{t}[i].{name}`.",
                code_block(&format!("{t}.{name}: {ty}"))
            )
        }
        DefKind::Fn if def.ty != T::Unknown => format!(
            "{}\nA value function, inlined at each call.",
            code_block(&format!("fn {}", fn_signature(m, d).0))
        ),
        DefKind::Fn => format!(
            "{}\nA function, inlined at each call. It has no value.",
            code_block(&format!("fn {}", fn_signature(m, d).0))
        ),
        DefKind::Const => {
            let value = match m.checked.constant(name) {
                Some((Type::Time, v)) => format!(" It is {v} ticks."),
                Some((_, v)) => format!(" It is {v}."),
                None => String::new(),
            };
            format!("{}\nA constant of type {ty}.{value}", decl())
        }
        DefKind::Param => {
            let f = def.parent.map_or("?", |p| m.defs[p].name.as_str());
            format!("{}\nA parameter of fn `{f}`.", code_block(&format!("(parameter) {name}: {ty}")))
        }
        DefKind::Arg => {
            let ev = def.event.map_or("?", Event::name);
            format!("{}\nAn argument of `on {ev}`.", code_block(&format!("(argument) {name}: {ty}")))
        }
        DefKind::Local => code_block(&format!("let {name}: {ty}")),
        DefKind::Binder => format!("{}\nA member of the set.", code_block(&format!("(member) {name}: {ty}"))),
        DefKind::Label => label_markdown(m, d),
        DefKind::Setting => match settings::named(name) {
            Some((index, _)) => setting_markdown(index, m.engine()),
            None => decl(),
        },
    }
}

/// A setting's hover: its name and kind, our label, the game types it
/// applies to, the values it takes and, for a script whose record is
/// another game's, that the script cannot name it.
pub(crate) fn setting_markdown(index: u16, engine: Option<u8>) -> String {
    let Some(row) = settings::row_at(index) else {
        return String::new();
    };
    let mut out = format!(
        "{}\n**{}**. A setting of the game variant. Applies to {}.",
        code_block(&format!("{}: {}", row.name, row.kind.word())),
        row.label,
        row.carrier_words()
    );
    let (lo, hi) = row.kind.range();
    let takes = match row.kind {
        SettingKind::Bool => "`true` or `false`.".to_string(),
        SettingKind::Int { .. } => format!("A number, {lo} to {hi}."),
        SettingKind::Seconds { .. } => format!("A time in whole seconds, {lo}s to {hi}s: `300s`."),
        SettingKind::Percent { .. } => format!("A percent, {lo} to {hi}."),
        SettingKind::Choice { names, .. } => {
            let list: Vec<String> = names.iter().map(|c| format!("`{}` ({})", c.name, c.label)).collect();
            format!("One of {}, or a number, {lo} to {hi}.", list.join(", "))
        }
    };
    out.push_str(&format!("\nTakes: {takes}"));
    if row.zero_is_none && row.choice("none").is_none() {
        out.push_str(" `none` is 0.");
    }
    if !row.is_record() {
        out.push_str("\nStored in the variant's extras (the `hx` block), not the game's own record.");
    }
    if let Some(e) = engine
        && !row.applies(e)
    {
        out.push_str(&format!(
            "\nThis script's record is {}'s, so it cannot name this setting.",
            settings::carrier_label(e)
        ));
    }
    out
}

/// A label's hover: its declaration, what the script sees, what a map
/// needs in Forge's words (`labels::describe`), and its uses.
fn label_markdown(m: &Model, d: usize) -> String {
    let def = &m.defs[d];
    let decl = code_block(m.line_at(def.span.0));
    let uses = m.refs.iter().filter(|r| r.def == d && !r.decl).count();
    let used = format!("Used {uses} {}.", if uses == 1 { "time" } else { "times" });
    let Some((_, l)) = m.checked.label(&def.name) else {
        return format!("{decl}\n{used}");
    };
    let what = if l.def.kind == LabelKind::Zone { "zone" } else { "object" };
    let sees = match (l.def.kind, l.single) {
        (LabelKind::Spawn, _) => "Spawn areas: pass it to `spawn_areas`; a loop cannot visit them.".to_string(),
        (_, true) if l.optional => format!("One {what}, or `none` on a map that has none (`{what}?`)."),
        (_, true) => format!("One {what}."),
        (_, false) => {
            let fields: Vec<&str> = FIELDS
                .iter()
                .filter(|(o, _, _)| *o == Ty::Object)
                .map(|(_, n, _)| *n)
                .collect();
            format!(
                "A set of {what}s. Members have {}, and this script's object variables.",
                fields.join(", ")
            )
        }
    };
    format!(
        "{decl}\n{sees}\nA map needs: {}.\n{used}",
        crate::labels::describe(&l.def)
    )
}

/// The hover of a use: its declaration's, and the type the checker gives
/// the name there when that is narrower than the declared one.
pub(crate) fn ref_markdown(m: &Model, r: Ref) -> String {
    let base = def_markdown(m, r.def);
    let def = &m.defs[r.def];
    let (T::V(declared), Some(here)) = (def.ty, r.ty) else {
        return base;
    };
    if r.decl || declared == here || def.kind == DefKind::Label {
        return base;
    }
    let not_none = declared.is_opt() && !here.is_opt();
    let why = match (r.narrowed, not_none) {
        (Some(line), true) => format!(" (not none here: checked on line {line})"),
        (Some(line), false) => format!(" (narrowed here: tested on line {line})"),
        (None, true) => " (not none here)".to_string(),
        (None, false) => String::new(),
    };
    format!("{base}\nHere `{}: {}`{why}.", def.name, here.name(&m.enum_names))
}

/// A non-declaration use's hover text; `s..e` is its span.
pub(crate) fn use_markdown(m: &Model, u: Use, s: usize, e: usize) -> String {
    let text: String = m.src[s..e].split_whitespace().collect();
    match u {
        Use::Api(entry) => format!("{}\n{}", code_block(&entry_signature(entry).0), entry_doc(entry)),
        Use::Lang(i) => {
            let l = &LANG[i];
            format!("{}\n{}", code_block(&lang_signature(l).0), l.doc)
        }
        Use::VariantField(i) => {
            let (n, t) = VARIANT_FIELDS[i];
            format!(
                "{}\nA field of the retail variant record.",
                code_block(&format!("variant.{n}: {}", t.name()))
            )
        }
        Use::Field(i) => {
            let (owner, n, id) = FIELDS[i];
            let entry = &caps::ENTRIES[id as usize];
            let result = m.type_name(super::model::builtin_field_type(i));
            format!(
                "{}\nA built-in field of every {}, read through `{}`.\n\nStatus: {}",
                code_block(&format!("{}.{n}: {result}", owner.name())),
                owner.name(),
                entry.name,
                status_words(entry.status)
            )
        }
        Use::Capability => match caps::capability(&text, caps::API_VERSION) {
            Some(c) => format!(
                "{}\nCapability: {}.{}",
                code_block(c.name),
                c.words,
                if c.grantable {
                    " A script may grant it to a player."
                } else {
                    ""
                }
            ),
            None => format!("Unknown capability `{text}`."),
        },
        Use::Event(ev) => {
            let (_, doc) = event_info(ev);
            format!("{}\n{doc}", code_block(&format!("on {}", event_signature(ev).0)))
        }
        Use::GameType(Some(t)) => {
            let row = t.row();
            format!(
                "{}\nThe {} game type: its variants are filed in the {} row, and a point is called {}. A record of this type is on engine index {}.",
                code_block(&format!("type {}", row.name)),
                row.label,
                row.label,
                row.points,
                row.engine
            )
        }
        Use::GameType(None) => format!("Unknown game type `{text}`."),
        Use::Type => match type_word_doc(&text) {
            Some(doc) => format!("{}\n{doc}", code_block(&text)),
            None => format!("Unknown type `{text}`."),
        },
        Use::Setting(index) => setting_markdown(index, m.engine()),
        Use::Variant => {
            let fields: Vec<String> = VARIANT_FIELDS
                .iter()
                .map(|(n, t)| format!("`{n}` ({})", t.name()))
                .collect();
            format!(
                "{}\nThe retail variant record: `variant.<field>` reads {}.",
                code_block("variant"),
                fields.join(", ")
            )
        }
    }
}

// ---------- diagnostics ----------

pub(crate) fn diag(range: Range, severity: u8, message: String, code: &'static str) -> Diagnostic {
    Diagnostic {
        range,
        severity,
        message,
        code,
        source: "rules",
        fixes: Vec::new(),
    }
}

/// The range of the token at a lexer line and column.
pub(crate) fn place(m: &Model, line: u32, col: u32) -> Range {
    let off = m.li.pos_offset(Pos { line, col });
    match m.code.get(m.ci(off)) {
        Some(t) if t.s == off => m.li.range(t.s, t.e),
        _ => m.li.range(off, off),
    }
}

pub(crate) fn diagnostics(m: &Model) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    // About the whole script: its first line.
    let whole = m.li.range(0, m.li.line_end(0));
    if m.src.len() > MAX_SOURCE_BYTES {
        let message = format!("the source is {} bytes, over the 256 KiB cap", m.src.len());
        out.push(diag(whole, severity::ERROR, message, "limit"));
        return out;
    }
    let mut syntax: Vec<_> = m.errors.iter().collect();
    syntax.sort_by_key(|e| (e.line, e.col));
    for e in syntax.into_iter().take(crate::lang::MAX_ERRORS) {
        out.push(diag(place(m, e.line, e.col), severity::ERROR, e.message.clone(), "syntax"));
    }
    if m.errors.is_empty() {
        // What `crate::diagnose` gives: the checker's every finding, then
        // lowering's limit when it has no error.
        for d in &m.checked.diagnostics {
            let level = match d.severity {
                Severity::Error => severity::ERROR,
                Severity::Warning => severity::WARNING,
            };
            let mut x = diag(place(m, d.pos.line, d.pos.col), level, d.message.clone(), d.code);
            x.fixes = super::fix::fixes(m, d);
            out.push(x);
        }
        if !m.checked.has_errors() {
            match crate::lang::lower::lower(&m.checked) {
                Ok(blob) => {
                    match crate::verify(&blob) {
                        Ok(r) if !r.fits() => {
                            let message = format!(
                                "the worst tick costs {} ops, over the budget of {}",
                                r.worst_tick,
                                crate::vm::TICK_BUDGET
                            );
                            out.push(diag(whole, severity::ERROR, message, "verify"));
                        }
                        Ok(_) => {}
                        Err(refusal) => out.push(diag(whole, severity::ERROR, format!("refused: {refusal}"), "verify")),
                    }
                    let bytes = blob.encode().len();
                    if bytes > ir::MAX_IR_BYTES {
                        let message = format!("the IR is {bytes} bytes, over the {} byte cap", ir::MAX_IR_BYTES);
                        out.push(diag(whole, severity::ERROR, message, "verify"));
                    }
                    unhostable(m, &crate::workbench::unhostable_all(&blob), &mut out);
                }
                Err(e) => out.push(diag(place(m, e.line, e.col), severity::ERROR, e.message, "limit")),
            }
        }
    }
    unused(m, &mut out);
    out
}

/// A note at each call of an entry in `names`, those no host can make on
/// the engine the script's record is on.
fn unhostable(m: &Model, names: &[&str], out: &mut Vec<Diagnostic>) {
    for &(s, e, u) in &m.uses {
        if let Use::Api(entry) = u
            && names.contains(&entry.name)
        {
            let message = match entry.hosts {
                Some(hosts) if entry.status.hostable() => format!(
                    "a host makes `{}` only on {}",
                    entry.name,
                    hosts
                        .iter()
                        .map(|&c| crate::caps::carrier_name(c).unwrap_or("?"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                _ => format!(
                    "no host can make `{}` yet: its entry is {}",
                    entry.name,
                    status_short(entry.status)
                ),
            };
            out.push(diag(m.li.range(s, e), severity::INFORMATION, message, "unhostable"));
        }
    }
}

/// A warning at each `let`, set member and `fn` the script never uses.
fn unused(m: &Model, out: &mut Vec<Diagnostic>) {
    let mut used = vec![0usize; m.defs.len()];
    for r in &m.refs {
        if !r.decl {
            used[r.def] += 1;
        }
    }
    for (d, def) in m.defs.iter().enumerate() {
        if used[d] > 0 || def.name.starts_with('_') {
            continue;
        }
        let message = match def.kind {
            DefKind::Local | DefKind::Binder => format!("`{}` is never used", def.name),
            DefKind::Fn => format!("fn `{}` is never called", def.name),
            _ => continue,
        };
        out.push(diag(
            m.li.range(def.span.0, def.span.1),
            severity::WARNING,
            message,
            "unused",
        ));
    }
}

// ---------- hover ----------

pub(crate) fn hover(m: &Model, off: usize) -> Option<Hover> {
    let ti = m.token_at(off)?;
    let t = m.code[ti];
    if let Some(r) = m.ref_at(t.s) {
        return Some(Hover {
            markdown: ref_markdown(m, r),
            range: m.li.range(r.s, r.e),
        });
    }
    if let Some((s, e, u)) = m.use_at(t.s) {
        return Some(Hover {
            markdown: use_markdown(m, u, s, e),
            range: m.li.range(s, e),
        });
    }
    let text = m.text(&t);
    let markdown = match t.k {
        K::Ident => match keyword_doc(text) {
            Some(doc) => format!("{}\n{doc}", code_block(text)),
            None => format!("{}\n{}", code_block(text), type_word_doc(text)?),
        },
        K::Num if text.ends_with(['s', 't']) => format!("{}\n{}", code_block("time"), time_literal_doc(text)),
        K::Num if text.contains('.') => format!("{}\nA 16.16 fixed literal.", code_block("fixed")),
        K::Num => format!("{}\nAn int literal (a fixed where one is expected).", code_block("int")),
        _ => return None,
    };
    Some(Hover {
        markdown,
        range: m.li.range(t.s, t.e),
    })
}

// ---------- signature help ----------

/// The call around the code tokens before index `end`: the index of the
/// called name and the commas before the place.
pub(crate) fn enclosing_call(m: &Model, end: usize) -> Option<(usize, u32)> {
    let mut depth = 0u32;
    let mut commas = 0u32;
    for j in (0..end.min(m.code.len())).rev().take(2048) {
        let t = m.code[j];
        if t.k != K::Punct {
            continue;
        }
        match m.text(&t) {
            ")" | "]" => depth += 1,
            "[" => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            "(" => {
                if depth == 0 {
                    let name = j.checked_sub(1)?;
                    return (m.code[name].k == K::Ident).then_some((name, commas));
                }
                depth -= 1;
            }
            "," if depth == 0 => commas += 1,
            "{" | "}" => return None,
            _ => {}
        }
    }
    None
}

pub(crate) fn signature_help(m: &Model, off: usize) -> Option<SignatureHelp> {
    let (ni, commas) = enclosing_call(m, m.ci(off))?;
    let name = m.text(&m.code[ni]);
    let (label, offsets, doc) = if ni > 0 && m.is_word(ni - 1, "on") {
        let ev = Event::named(name)?;
        let (l, o) = event_signature(ev);
        (l, o, event_info(ev).1.to_string())
    } else if let Some(d) = m.top_kind(name, DefKind::Fn) {
        let (l, o) = fn_signature(m, d);
        (l, o, "A function, inlined at each call.".to_string())
    } else if let Some(i) = lang_index(name) {
        let (l, o) = lang_signature(&LANG[i]);
        (l, o, LANG[i].doc.to_string())
    } else {
        let e = caps::entry_named(name)?;
        let (l, o) = entry_signature(e);
        (l, o, entry_doc(e))
    };
    let units: Vec<u16> = label.encode_utf16().collect();
    let parameters: Vec<Parameter> = offsets
        .iter()
        .map(|o| Parameter {
            label: String::from_utf16_lossy(&units[o[0] as usize..o[1] as usize]),
            offsets: *o,
        })
        .collect();
    let n = parameters.len() as u32;
    Some(SignatureHelp {
        signatures: vec![Signature {
            label,
            documentation: Markup::markdown(doc),
            parameters,
        }],
        active_signature: 0,
        active_parameter: if n == 0 { 0 } else { commas.min(n - 1) },
    })
}

// ---------- definitions and references ----------

/// The resolved name at `off`, or the one ending there.
pub(crate) fn ref_near(m: &Model, off: usize) -> Option<Ref> {
    m.ref_at(off)
        .or_else(|| off.checked_sub(1).and_then(|o| m.ref_at(o)))
}

pub(crate) fn definition(m: &Model, off: usize) -> Vec<Location> {
    let Some(r) = ref_near(m, off) else {
        return Vec::new();
    };
    let (s, e) = m.defs[r.def].span;
    vec![Location {
        range: m.li.range(s, e),
    }]
}

pub(crate) fn references(m: &Model, off: usize) -> Vec<Location> {
    let Some(r) = ref_near(m, off) else {
        return Vec::new();
    };
    let mut refs: Vec<&Ref> = m.refs.iter().filter(|x| x.def == r.def).collect();
    refs.sort_by_key(|x| (!x.decl, x.s));
    refs.iter()
        .map(|x| Location {
            range: m.li.range(x.s, x.e),
        })
        .collect()
}

pub(crate) fn highlights(m: &Model, off: usize) -> Vec<Highlight> {
    let Some(r) = ref_near(m, off) else {
        return Vec::new();
    };
    let mut refs: Vec<&Ref> = m.refs.iter().filter(|x| x.def == r.def).collect();
    refs.sort_by_key(|x| x.s);
    refs.dedup_by_key(|x| (x.s, x.e));
    refs.iter()
        .map(|x| {
            // A name right before `=` (not `==`, one token) is assigned.
            let write = x.decl || m.is_punct(m.ci(x.e), "=");
            Highlight {
                range: m.li.range(x.s, x.e),
                kind: if write { highlight_kind::WRITE } else { highlight_kind::READ },
            }
        })
        .collect()
}

// ---------- semantic tokens ----------

fn def_token(kind: DefKind) -> TokenKind {
    match kind {
        DefKind::Option
        | DefKind::Var(_)
        | DefKind::Table
        | DefKind::Column
        | DefKind::Local
        | DefKind::Binder => TokenKind::Variable,
        DefKind::Palette => TokenKind::String,
        DefKind::Enum => TokenKind::Enum,
        DefKind::Member => TokenKind::EnumMember,
        DefKind::Fn => TokenKind::Function,
        DefKind::Param | DefKind::Arg => TokenKind::Parameter,
        DefKind::Label => TokenKind::Label,
        DefKind::Const | DefKind::Setting => TokenKind::Variable,
    }
}

fn use_token(u: Use) -> TokenKind {
    match u {
        Use::Api(_) => TokenKind::Api,
        Use::Lang(_) => TokenKind::Function,
        Use::VariantField(_) | Use::Field(_) => TokenKind::Variable,
        Use::Capability => TokenKind::Capability,
        Use::Event(_) => TokenKind::Event,
        Use::GameType(_) => TokenKind::EnumMember,
        Use::Type => TokenKind::Type,
        Use::Variant => TokenKind::Keyword,
        Use::Setting(_) => TokenKind::Variable,
    }
}

pub(crate) fn semantic_tokens(m: &Model) -> Vec<Token> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<Token>, s: usize, e: usize, kind: TokenKind| {
        let p: Position = m.li.position(s);
        out.push(Token {
            line: p.line,
            start: p.character,
            len: m.src[s..e].encode_utf16().count() as u32,
            kind,
        });
    };
    let mut skip_to = 0usize;
    let mut prev_colon = false;
    let header: Vec<usize> = m.header_words().into_iter().map(|i| m.code[i].s).collect();
    for t in &m.all {
        if t.s < skip_to {
            continue;
        }
        let text = &m.src[t.s..t.e];
        let kind = match t.k {
            K::Comment => Some(TokenKind::Comment),
            K::Num => Some(TokenKind::Number),
            K::Str => match m.ref_at(t.s) {
                Some(r) if m.defs[r.def].kind == DefKind::Label => Some(TokenKind::Label),
                _ => Some(TokenKind::String),
            },
            K::Punct => OPERATORS.contains(&text).then_some(TokenKind::Operator),
            K::Bad => None,
            K::Ident => {
                if header.contains(&t.s) {
                    Some(TokenKind::Keyword)
                } else if let Some((s, e, u)) = m.use_at(t.s) {
                    if matches!(u, Use::Capability) && e > t.e && m.li.line_of(s) == m.li.line_of(e) {
                        push(&mut out, s, e, TokenKind::Capability);
                        skip_to = e;
                        prev_colon = false;
                        continue;
                    }
                    Some(use_token(u))
                } else if let Some(r) = m.ref_at(t.s) {
                    Some(def_token(m.defs[r.def].kind))
                } else if prev_colon && is_type_word(text) {
                    Some(TokenKind::Type)
                } else if is_keyword(text) {
                    Some(TokenKind::Keyword)
                } else {
                    Some(TokenKind::Variable)
                }
            }
        };
        if t.k != K::Comment {
            prev_colon = t.k == K::Punct && text == ":";
        }
        if let Some(k) = kind {
            push(&mut out, t.s, t.e, k);
        }
    }
    out
}

// ---------- symbols ----------

fn contained(inner: Range, outer: Range) -> bool {
    inner.start >= outer.start && inner.end <= outer.end
}

pub(crate) fn symbols(m: &Model) -> Vec<Symbol> {
    let mut out = Vec::new();
    let script = &m.script;
    if !script.name.is_empty() {
        let off = m.li.pos_offset(script.pos);
        let range = m.li.range(off, m.li.line_end(m.li.line_of(off)));
        out.push(Symbol {
            name: script.name.clone(),
            detail: format!("rules, version {}", script.version),
            kind: symbol_kind::MODULE,
            range,
            selection_range: range,
            children: Vec::new(),
        });
    }
    let leaf = |d: usize, detail: String, kind: u8| {
        let r = m.li.range(m.defs[d].span.0, m.defs[d].span.1);
        Symbol {
            name: m.defs[d].name.clone(),
            detail,
            kind,
            range: r,
            selection_range: r,
            children: Vec::new(),
        }
    };
    for (i, item) in script.items.iter().enumerate() {
        let Some(&(s, e)) = m.items.get(i) else { break };
        let range = m.li.range(s, e);
        let d = m.item_defs.get(i).copied().flatten();
        let mut selection = d.map_or(range, |d| m.li.range(m.defs[d].span.0, m.defs[d].span.1));
        let (name, detail, kind, children) = match item {
            Item::Option(o) => (
                o.name.clone(),
                format!("option: {}", o.ty.name),
                symbol_kind::PROPERTY,
                Vec::new(),
            ),
            Item::Palette { name, tags, .. } => (
                format!("palette \"{name}\""),
                format!("{} tags", tags.len()),
                symbol_kind::ARRAY,
                Vec::new(),
            ),
            Item::Enum { name, .. } => (
                name.clone(),
                "enum".to_string(),
                symbol_kind::ENUM,
                d.map_or_else(Vec::new, |d| {
                    m.children(d, DefKind::Member)
                        .map(|c| leaf(c, String::new(), symbol_kind::ENUM_MEMBER))
                        .collect()
                }),
            ),
            Item::Var { scope, name, ty, .. } => {
                let word = match scope {
                    Scope::Global => "global",
                    Scope::Player => "player",
                    Scope::Team => "team",
                    Scope::Object => "object",
                };
                (
                    name.clone(),
                    format!("{word} var: {}", ty.name),
                    symbol_kind::VARIABLE,
                    Vec::new(),
                )
            }
            Item::Table { name, rows, .. } => (
                name.clone(),
                format!("table, {} rows", rows.len()),
                symbol_kind::STRUCT,
                d.map_or_else(Vec::new, |d| {
                    m.children(d, DefKind::Column)
                        .map(|c| leaf(c, m.type_name(m.defs[c].ty), symbol_kind::FIELD))
                        .collect()
                }),
            ),
            Item::Fn(f) => (
                f.name.clone(),
                d.map_or_else(String::new, |d| format!("fn {}", fn_signature(m, d).0)),
                symbol_kind::FUNCTION,
                Vec::new(),
            ),
            Item::Label(l) => (
                l.name.clone(),
                format!("label: {}{}", l.kind.name, if l.kind.optional { "?" } else { "" }),
                symbol_kind::CONSTANT,
                Vec::new(),
            ),
            Item::Const { name, .. } => (
                name.clone(),
                d.map_or_else(|| "const".to_string(), |d| format!("const: {}", m.type_name(m.defs[d].ty))),
                symbol_kind::CONSTANT,
                Vec::new(),
            ),
            Item::On { event, args, .. } => {
                if let Some((us, ue, _)) = m.uses.iter().copied().find(|u| u.0 >= s && u.0 < e && matches!(u.2, Use::Event(_))) {
                    selection = m.li.range(us, ue);
                }
                (
                    format!("on {event}"),
                    if args.is_empty() {
                        String::new()
                    } else {
                        format!("({})", args.join(", "))
                    },
                    symbol_kind::EVENT,
                    Vec::new(),
                )
            }
        };
        if !contained(selection, range) {
            selection = range;
        }
        let children = children.into_iter().filter(|c| contained(c.range, range)).collect();
        out.push(Symbol {
            name,
            detail,
            kind,
            range,
            selection_range: selection,
            children,
        });
    }
    nest_sections(m, out)
}

/// The declarations after a `section "Name"` line, under a symbol of its
/// own that runs to the next section (docs/game/rules-world-api.md,
/// "Splitting a script"); those before the first stay at the top.
fn nest_sections(m: &Model, symbols: Vec<Symbol>) -> Vec<Symbol> {
    if m.script.sections.is_empty() {
        return symbols;
    }
    let mut marks: Vec<Symbol> = m
        .script
        .sections
        .iter()
        .map(|(name, pos)| {
            let off = m.li.pos_offset(*pos);
            let range = m.li.range(off, m.li.line_end(m.li.line_of(off)));
            Symbol {
                name: name.clone(),
                detail: "section".to_string(),
                kind: symbol_kind::NAMESPACE,
                range,
                selection_range: range,
                children: Vec::new(),
            }
        })
        .collect();
    marks.sort_by_key(|s| s.range.start.line);
    let lines: Vec<u32> = marks.iter().map(|s| s.range.start.line).collect();
    let mut top = Vec::new();
    for sym in symbols {
        match lines.iter().rposition(|&l| l <= sym.range.start.line) {
            Some(i) => {
                marks[i].range.end = sym.range.end;
                marks[i].children.push(sym);
            }
            None => top.push(sym),
        }
    }
    top.extend(marks);
    top
}

/// The type words a declaration may spell (the enums aside): the
/// front-end types, `timer`, and the set words a parameter takes.
pub(crate) fn type_names() -> &'static [&'static str] {
    &[
        "int", "fixed", "time", "bool", "player", "team", "object", "zone", "squad", "timer", "Class", "tag", "weapon",
        "text", "objects", "zones", "spawns",
    ]
}

/// The doc of a type a name spells, built-in or enum.
pub(crate) fn type_named_doc(m: &Model, name: &str) -> String {
    match type_word_doc(name) {
        Some(doc) => doc.to_string(),
        None => match m.top_kind(name, DefKind::Enum) {
            Some(d) => def_markdown(m, d),
            None => String::new(),
        },
    }
}

