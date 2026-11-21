//! What the language's names mean (the docs hover and completion show),
//! and the features that read the model directly: diagnostics, hover,
//! signature help, definitions, references, semantic tokens and the
//! outline.

use super::model::{DefKind, Model, Ref, Use, builtin_type};
use super::scan::K;
use super::{
    Diagnostic, Hover, Location, Markup, Parameter, Position, Range, Signature, SignatureHelp, Symbol, Token,
    TokenKind, severity, symbol_kind,
};
use crate::caps::{self, CARRIERS, Entry, Kind, Order, Status, VARIANT_FIELDS};
use crate::ir::{Event, Scope};
use crate::lang::Pos;
use crate::lang::ast::Item;
use crate::lang::lower::FIELDS;
use crate::lang::parse::KEYWORDS;
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
        result: "int",
        doc: "`n` seconds in ticks (30 a second), for timers.",
        snippet: "seconds(${1:1})",
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
const P_SPOT: &[&str] = &["p", "spot"];
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
        Event::Bounce => (P_SPOT, "A player superbounced."),
        Event::Edit => (P_O, "A player used a granted power."),
        Event::Join => (P, "A player joined."),
        Event::Quit => (P, "A player quit."),
        Event::RoundEnd => (NO_ARGS, "At the end of each round."),
    }
}

/// `kill(victim: player, killer: player)` and each argument's offsets.
pub(crate) fn event_signature(ev: Event) -> (String, Vec<[u32; 2]>) {
    let (names, _) = event_info(ev);
    let tys: Vec<&str> = if ev == Event::Timer {
        vec!["timer"]
    } else {
        ev.args().iter().map(|t| t.name()).collect()
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

pub(crate) fn entry_signature(e: &Entry) -> (String, Vec<[u32; 2]>) {
    call_label(caps::spelled(e), &caps::params(e), e.result.map_or("", Ty::name))
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
    call_label(&m.defs[d].name, &params, "")
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

pub(crate) fn type_doc(t: Ty) -> &'static str {
    match t {
        Ty::Int => "A 32-bit integer; arithmetic saturates.",
        Ty::Fixed => "A 16.16 fixed-point number, for seconds and distances.",
        Ty::Bool => "`true` or `false`.",
        Ty::Player => "A player (a roster slot), or `none`.",
        Ty::Team => "A team (0 to 7), or `none`.",
        Ty::Object => "An object handle the game hands out, or `none`.",
        Ty::Squad => "A kit squad slot, 0 to 15: `squad(n)`.",
        Ty::Timer => "A global variable that counts ticks down; 0 is stopped. Reaching 0 fires `on timer`.",
        Ty::Class => "A kit class name.",
        Ty::Tag => "A tag path, checked against the map at load.",
        Ty::Label => "A Forge label.",
        Ty::Text => "A message: `text \"...\"`.",
        Ty::Enum(_) => "A script-declared enum.",
    }
}

pub(crate) fn keyword_doc(w: &str) -> Option<&'static str> {
    Some(match w {
        "rules" => "The header: `rules \"Name\" version`.",
        "carrier" => "The retail engine the script rides: `carrier slayer`.",
        "teams" => "In the header, the team count (1 to 8): `teams 2`. In a set, every team.",
        "requires" => "The capabilities the script declares: `requires player.weapons, hud.message`. The lobby shows them in plain words.",
        "option" => "A row of the variant editor: `option name: int 1..5 = 1 \"Label\" \"Description\"`. An int, a bool or an enum, read-only in handlers.",
        "palette" => "A named list of tag paths: `palette \"x\" = [\"objects/...\"]`. `palette \"x\"` is a `grant` argument.",
        "enum" => "A script-declared type: `enum Role { none, vip }`, read as `Role.vip`.",
        "global" | "var" => "A variable: `global|player|team|object var name: type = init`.",
        "player" => "A player, or `none`. `player var` declares a variable of each player.",
        "team" => "A team, or `none`; `team 0` is a team value. `team var` declares a variable of each team.",
        "object" => "An object handle, or `none`. `object var` declares a variable of each labelled object.",
        "table" => "Constant rows of typed columns: `table t: (k: int, v: int) = [(0, 1)]`; `t[i].v` reads a cell.",
        "fn" => "A function: inlined at each call. It has no value and does not recurse.",
        "on" => "A handler: `on kill(victim, killer) { ... }`. It runs to completion.",
        "if" | "else" => "`if cond { } else if cond { } else { }`.",
        "for" | "each" => "A loop over a finite set: `for each players p where p.alive { }`.",
        "where" => "A set's filter. It may read the member's fields without naming it.",
        "let" => "A name for a value, not assigned again: `let n = count(players)`.",
        "and" | "or" => "Boolean operators; both short-circuit, and `and` binds tighter than `or`.",
        "not" => "Boolean negation.",
        "true" | "false" => "A `bool`.",
        "none" => "No player, team, object or tag.",
        "labelled" => "A Forge label: `labelled \"x\"`. `objects labelled \"x\"` is a set.",
        "text" => "A message: `text \"...\"`, or a string-table index.",
        "budget" => "A grant's uses: `budget 20`.",
        "players" => "The set of players, by slot.",
        "objects" => "`objects labelled \"x\"`: the objects with a Forge label, by placement.",
        "objectives" => "The objective registry's objects, at most 8, by handle.",
        "variant" => "The retail variant record: `variant.<field>` reads a field.",
        _ => return None,
    })
}

/// A word the semantic tokens call a keyword when nothing else claims it.
pub(crate) fn is_keyword(w: &str) -> bool {
    KEYWORDS.contains(&w) || matches!(w, "player" | "object" | "players" | "objects" | "objectives" | "variant")
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
        DefKind::Fn => format!(
            "{}\nA function, inlined at each call. It has no value.",
            code_block(&format!("fn {}", fn_signature(m, d).0))
        ),
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
        DefKind::Label => {
            let uses = m.refs.iter().filter(|r| r.def == d).count();
            format!(
                "{}\nA Forge label, used {uses} {} in this script.",
                code_block(&format!("labelled \"{name}\"")),
                if uses == 1 { "time" } else { "times" }
            )
        }
    }
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
            let result = entry.result.map_or("?", Ty::name);
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
        Use::Carrier => match CARRIERS.iter().find(|(n, _)| *n == text) {
            Some((n, index)) => format!(
                "{}\nThe script rides the retail {n} engine (engine index {index}).",
                code_block(&format!("carrier {n}"))
            ),
            None => format!("Unknown carrier `{text}`."),
        },
        Use::Type => match builtin_type(&text) {
            Some(t) => format!("{}\n{}", code_block(t.name()), type_doc(t)),
            None => format!("Unknown type `{text}`."),
        },
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

fn diag(range: Range, severity: u8, message: String, code: &'static str) -> Diagnostic {
    Diagnostic {
        range,
        severity,
        message,
        code,
        source: "rules",
    }
}

/// The range of the token at a lexer line and column.
fn place(m: &Model, line: u32, col: u32) -> Range {
    let off = m.li.pos_offset(Pos { line, col });
    match m.code.get(m.ci(off)) {
        Some(t) if t.s == off => m.li.range(t.s, t.e),
        _ => m.li.range(off, off),
    }
}

pub(crate) fn diagnostics(m: &Model) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for e in &m.errors {
        out.push(diag(place(m, e.line, e.col), severity::ERROR, e.message.clone(), "syntax"));
    }
    if m.errors.is_empty() {
        let check = crate::workbench::check(m.src);
        for p in &check.errors {
            if p.line == 0 {
                // About the whole script: its first line.
                let range = m.li.range(0, m.li.line_end(0));
                out.push(diag(range, severity::ERROR, p.message.clone(), "verify"));
            } else {
                out.push(diag(place(m, p.line, p.col), severity::ERROR, p.message.clone(), "compile"));
            }
        }
        if let Some(report) = &check.report {
            for &(s, e, u) in &m.uses {
                if let Use::Api(entry) = u
                    && report.unhostable.contains(&entry.name)
                {
                    out.push(diag(
                        m.li.range(s, e),
                        severity::INFORMATION,
                        format!(
                            "no host can make `{}` yet: its entry is {}",
                            entry.name,
                            status_short(entry.status)
                        ),
                        "unhostable",
                    ));
                }
            }
        }
    }
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
    out
}

// ---------- hover ----------

pub(crate) fn hover(m: &Model, off: usize) -> Option<Hover> {
    let ti = m.token_at(off)?;
    let t = m.code[ti];
    if let Some(r) = m.ref_at(t.s) {
        return Some(Hover {
            markdown: def_markdown(m, r.def),
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
            None => {
                let ty = builtin_type(text)?;
                format!("{}\n{}", code_block(ty.name()), type_doc(ty))
            }
        },
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
    } else if let Some(e) = caps::entry_named(name) {
        let (l, o) = entry_signature(e);
        (l, o, entry_doc(e))
    } else {
        return None;
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
fn ref_near(m: &Model, off: usize) -> Option<Ref> {
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
    }
}

fn use_token(u: Use) -> TokenKind {
    match u {
        Use::Api(_) => TokenKind::Api,
        Use::Lang(_) => TokenKind::Function,
        Use::VariantField(_) | Use::Field(_) => TokenKind::Variable,
        Use::Capability => TokenKind::Capability,
        Use::Event(_) => TokenKind::Event,
        Use::Carrier => TokenKind::EnumMember,
        Use::Type => TokenKind::Type,
        Use::Variant => TokenKind::Keyword,
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
                if let Some((s, e, u)) = m.use_at(t.s) {
                    if matches!(u, Use::Capability) && e > t.e && m.li.line_of(s) == m.li.line_of(e) {
                        push(&mut out, s, e, TokenKind::Capability);
                        skip_to = e;
                        prev_colon = false;
                        continue;
                    }
                    Some(use_token(u))
                } else if let Some(r) = m.ref_at(t.s) {
                    Some(def_token(m.defs[r.def].kind))
                } else if prev_colon && builtin_type(text).is_some() {
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
    out
}

/// The types a declaration may name: the built-ins and the enums.
pub(crate) fn type_names() -> &'static [&'static str] {
    &[
        "int", "fixed", "bool", "player", "team", "object", "squad", "timer", "Class", "tag", "label", "text",
    ]
}

/// The doc of a type a name spells, built-in or enum.
pub(crate) fn type_named_doc(m: &Model, name: &str) -> String {
    match builtin_type(name) {
        Some(t) => type_doc(t).to_string(),
        None => match m.top_kind(name, DefKind::Enum) {
            Some(d) => def_markdown(m, d),
            None => String::new(),
        },
    }
}

