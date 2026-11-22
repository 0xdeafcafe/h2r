//! Completion by context: what the tokens before the place say may come
//! next, ranked by the type the place wants (`lang::sig` for an API
//! argument, a parameter's declared type for a `fn`'s). Nothing is
//! hidden; what fits comes first.

use super::info::{self, LANG};
use super::model::{self, DefKind, FieldTarget, Model, T};
use super::scan::{self, K};
use super::{CompletionItem, Markup, PLAIN_TEXT, SNIPPET, kind};
use crate::caps::{self, CAPABILITIES, CARRIERS, ENTRIES, VARIANT_FIELDS};
use crate::ir::{Event, Scope};
use crate::lang::lower::FIELDS;
use crate::lang::sig;
use crate::lang::types::{Handle, Type};

/// The netgame flags' built-in labels (`forge_doc::labels::builtin_label`),
/// each with the kind and requirements the built-in scripts declare it
/// with, offered whole at the name of a new `label` line.
const FLAG_LABELS: &[(&str, &str)] = &[
    ("ctf_flag_spawn", "zones at most 16"),
    ("ctf_flag_return", "zones at most 16"),
    ("assault_bomb_spawn", "zones at most 16"),
    ("assault_bomb_return", "zones at most 16"),
    ("oddball_spawn", "objects"),
    ("teleporter_sender", "zones"),
    ("teleporter_receiver", "zones"),
    ("territory", "zones 1..8"),
    ("hill_corner", "zones numbered"),
];

fn item(
    label: impl Into<String>,
    kind: u8,
    detail: impl Into<String>,
    doc: impl Into<String>,
    insert: impl Into<String>,
    format: u8,
) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind,
        detail: detail.into(),
        documentation: Markup::markdown(doc),
        insert_text: insert.into(),
        insert_text_format: format,
        sort_text: String::new(),
    }
}

fn plain(label: &str, kind: u8, detail: impl Into<String>, doc: impl Into<String>) -> CompletionItem {
    item(label, kind, detail, doc, label, PLAIN_TEXT)
}

/// The completions at `off`, each with its place in the list as its
/// `sort_text`.
pub(crate) fn complete(m: &Model, off: usize) -> Vec<CompletionItem> {
    let mut out = candidates(m, off);
    for (n, c) in out.iter_mut().enumerate() {
        c.sort_text = format!("{n:04}");
    }
    out
}

fn candidates(m: &Model, off: usize) -> Vec<CompletionItem> {
    if m.all
        .iter()
        .any(|t| t.k == K::Comment && t.s < off && off <= t.e)
    {
        return Vec::new();
    }
    // Inside a string: a label or a palette name, without quotes.
    if let Some(si) = m.code.iter().position(|t| {
        t.k == K::Str && t.s < off && (off < t.e || (off == t.e && !scan::closed(m.src, t)))
    }) {
        return match si.checked_sub(1).map(|j| m.text(&m.code[j])) {
            Some("palette") => palettes(m, false),
            _ => Vec::new(),
        };
    }
    // `i` is the word being typed, or the first token after the place.
    let mut i = m.ci(off);
    if let Some(j) = i.checked_sub(1)
        && m.code[j].k == K::Ident
        && m.code[j].e >= off
    {
        i = j;
    }
    let prev = i.checked_sub(1);
    let ptext = prev.map_or("", |j| m.text(&m.code[j]));
    let line = m.li.line_of(off);
    let line_start = prev.is_none_or(|j| m.li.line_of(m.code[j].s) < line);
    let first_on_line = (0..i)
        .rev()
        .take_while(|&j| m.li.line_of(m.code[j].s) == line)
        .last();
    // Brace depth; an enum's braces hold no expressions.
    let mut braces: Vec<bool> = Vec::new();
    for j in 0..i {
        if m.is_punct(j, "{") {
            braces.push(j >= 2 && m.is_word(j - 2, "enum"));
        } else if m.is_punct(j, "}") {
            braces.pop();
        }
    }
    if braces.last() == Some(&true) {
        return Vec::new();
    }
    let depth = braces.len();
    let in_requires = depth == 0 && first_on_line.is_some_and(|f| m.is_word(f, "requires"));

    if let Some(dot) = prev
        && ptext == "."
    {
        return after_dot(m, dot, off, in_requires);
    }
    // A `label` line: whole flag labels at the name, the kinds after `:`,
    // then the requirements.
    if depth == 0
        && let Some(f) = first_on_line
        && m.is_word(f, "label")
    {
        if prev == Some(f) {
            return flag_labels();
        }
        if ptext == ":" {
            return label_kinds();
        }
        if let Some(colon) = (f..i).find(|&j| m.is_punct(j, ":"))
            && prev.is_some_and(|p| p > colon)
            && !(colon..i).any(|j| m.is_punct(j, "="))
        {
            let set = m
                .code
                .get(colon + 1)
                .is_some_and(|t| matches!(m.text(t), "objects" | "zones" | "spawns"));
            return requirements(set);
        }
        return Vec::new();
    }
    // A label in its place among a handler's arguments: `on enter(`.
    if depth == 0
        && let Some((ni, commas)) = info::enclosing_call(m, i)
        && ni > 0
        && m.is_word(ni - 1, "on")
    {
        let slot = Event::named(m.text(&m.code[ni])).and_then(sig::label_arg);
        return match slot {
            Some((k, h)) if commas as usize == k => labels_fitting(m, |t| member_fits(t, h)),
            _ => Vec::new(),
        };
    }
    if depth > 0 && ptext == "is" {
        return labels_fitting(m, |t| member_fits(t, Handle::Object));
    }
    if depth == 0 && ptext == "on" {
        return events(m, "");
    }
    if ptext == "carrier" {
        return carriers();
    }
    if in_requires {
        return capabilities("", false);
    }
    if ptext == ":" {
        return types(m);
    }
    if ptext == "palette" && depth > 0 {
        return palettes(m, true);
    }
    let set_call = ptext == "("
        && prev
            .and_then(|j| j.checked_sub(1))
            .is_some_and(|k| matches!(m.text(&m.code[k]), "count" | "first" | "pick"));
    if ptext == "each" || set_call {
        let mut out = sets();
        out.extend(labels(m));
        return out;
    }
    if depth == 0 {
        if line_start {
            return top_level(m);
        }
        if prev.is_some() && prev == first_on_line && matches!(ptext, "global" | "player" | "team" | "object") {
            return vec![plain(
                "var",
                kind::KEYWORD,
                "",
                info::keyword_doc("var").unwrap_or(""),
            )];
        }
        if matches!(ptext, "=" | ".." | "-") {
            return constants(m);
        }
        return Vec::new();
    }
    // A capability argument of grant, revoke or granted.
    if let Some((ni, commas)) = info::enclosing_call(m, i)
        && commas == 1
        && matches!(m.text(&m.code[ni]), "grant" | "revoke" | "granted")
    {
        return capabilities("", true);
    }
    let mut out = match wanted(m, i) {
        Some(want) => ranked(m, off, want),
        None => expression(m, off),
    };
    if line_start || matches!(ptext, "{" | ";") {
        out.extend(statements());
    }
    if ptext == "}" {
        out.push(plain("else", kind::KEYWORD, "", info::keyword_doc("else").unwrap_or("")));
    }
    out
}

/// After `x.`: capability names, `variant` fields, enum variants, table
/// columns, or a value's variables and built-in fields.
fn after_dot(m: &Model, dot: usize, off: usize, in_requires: bool) -> Vec<CompletionItem> {
    let Some(before) = dot.checked_sub(1) else {
        return Vec::new();
    };
    // `t[i].`: a table row.
    if m.is_punct(before, "]") {
        if let Some(open) = m.partner(before)
            && let Some(n) = open.checked_sub(1)
            && let Some(td) = m.top_kind(m.text(&m.code[n]), DefKind::Table)
        {
            return fields_of(m, T::Row(td));
        }
        return Vec::new();
    }
    // The dotted path before the dot, as `a.b.c`.
    let mut parts: Vec<&str> = Vec::new();
    let mut j = before;
    loop {
        if m.code[j].k != K::Ident {
            break;
        }
        parts.push(m.text(&m.code[j]));
        match j.checked_sub(2) {
            Some(k) if m.is_punct(j - 1, ".") => j = k,
            _ => break,
        }
    }
    if parts.is_empty() {
        return Vec::new();
    }
    parts.reverse();
    let head = parts[0];
    let path = parts.join(".");
    let value = m.type_of_name(head, off) != T::Unknown;
    let is_cap_path = CAPABILITIES
        .iter()
        .any(|c| c.name.starts_with(&format!("{path}.")));
    if in_requires || (is_cap_path && !value && m.top(head).is_none()) {
        return capabilities(&format!("{path}."), false);
    }
    if parts.len() == 1 && m.visible_locals(off).iter().all(|&d| m.defs[d].name != head) {
        if head == "variant" {
            return VARIANT_FIELDS
                .iter()
                .map(|(n, t)| {
                    plain(
                        n,
                        kind::PROPERTY,
                        t.name(),
                        format!("A field of the retail variant record: `variant.{n}`, a `{}`.", t.name()),
                    )
                })
                .collect();
        }
        if let Some(ed) = m.top_kind(head, DefKind::Enum) {
            return m
                .children(ed, DefKind::Member)
                .map(|c| {
                    plain(
                        &m.defs[c].name,
                        kind::ENUM_MEMBER,
                        head,
                        info::def_markdown(m, c),
                    )
                })
                .collect();
        }
    }
    let mut t = m.type_of_name(head, off);
    for f in &parts[1..] {
        t = match model::field_target(m, t, f) {
            Some(FieldTarget::Def(d)) => m.defs[d].ty,
            Some(FieldTarget::Builtin(b)) => model::builtin_field_type(b),
            None => T::Unknown,
        };
    }
    fields_of(m, t)
}

/// The fields of a value of type `t`.
fn fields_of(m: &Model, t: T) -> Vec<CompletionItem> {
    let mut out = Vec::new();
    match t {
        T::V(Type::H(h, _)) => {
            let ty = h.erase();
            if let Some(scope) = model::scope_of(h) {
                for (d, def) in m.defs.iter().enumerate() {
                    if def.kind == DefKind::Var(scope) {
                        out.push(plain(
                            &def.name,
                            kind::FIELD,
                            m.type_name(def.ty),
                            info::def_markdown(m, d),
                        ));
                    }
                }
            }
            for (i, &(owner, name, id)) in FIELDS.iter().enumerate() {
                if owner == ty {
                    let entry = &ENTRIES[id as usize];
                    out.push(plain(
                        name,
                        kind::PROPERTY,
                        m.type_name(model::builtin_field_type(i)),
                        format!(
                            "A built-in field of every {}, read through `{}`.\n\nStatus: {}",
                            owner.name(),
                            entry.name,
                            info::status_words(entry.status)
                        ),
                    ));
                }
            }
        }
        T::Row(td) => {
            for c in m.children(td, DefKind::Column) {
                out.push(plain(
                    &m.defs[c].name,
                    kind::FIELD,
                    m.type_name(m.defs[c].ty),
                    info::def_markdown(m, c),
                ));
            }
        }
        _ => {}
    }
    out
}

/// Placeholders `${1:a}, ${2:b}`.
fn placeholders(names: &[String]) -> String {
    names
        .iter()
        .enumerate()
        .map(|(k, n)| format!("${{{}:{}}}", k + 1, n))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every event, with its arguments as a snippet; `prefix` goes before
/// each (`"on "` at the top level).
fn events(m: &Model, prefix: &str) -> Vec<CompletionItem> {
    let timer = m
        .defs
        .iter()
        .find(|d| d.kind == DefKind::Var(Scope::Global) && d.ty == T::Timer)
        .map_or("t".to_string(), |d| d.name.clone());
    Event::ALL
        .iter()
        .map(|&ev| {
            let (names, doc) = info::event_info(ev);
            let names: Vec<String> = if ev == Event::Timer {
                vec![timer.clone()]
            } else {
                names.iter().map(|n| n.to_string()).collect()
            };
            let args = if names.is_empty() {
                String::new()
            } else {
                format!("({})", placeholders(&names))
            };
            item(
                format!("{prefix}{}", ev.name()),
                kind::EVENT,
                format!("on {}", info::event_signature(ev).0),
                doc,
                format!("{prefix}{}{args} {{\n\t$0\n}}", ev.name()),
                SNIPPET,
            )
        })
        .collect()
}

fn carriers() -> Vec<CompletionItem> {
    CARRIERS
        .iter()
        .map(|(n, index)| {
            plain(
                n,
                kind::ENUM_MEMBER,
                format!("engine index {index}"),
                format!("The script rides the retail {n} engine."),
            )
        })
        .collect()
}

/// Capabilities whose names start with `prefix`, completed past it;
/// only the grantable ones when `grantable`.
fn capabilities(prefix: &str, grantable: bool) -> Vec<CompletionItem> {
    CAPABILITIES
        .iter()
        .filter(|c| c.name.starts_with(prefix) && (!grantable || c.grantable))
        .map(|c| {
            let rest = &c.name[prefix.len()..];
            let doc = format!(
                "{}.{}",
                c.words,
                if c.grantable {
                    " A script may grant it to a player."
                } else {
                    ""
                }
            );
            item(rest, kind::VALUE, c.words, doc, rest, PLAIN_TEXT)
        })
        .collect()
}

fn types(m: &Model) -> Vec<CompletionItem> {
    let mut out: Vec<CompletionItem> = info::type_names()
        .iter()
        .map(|n| plain(n, kind::TYPE_PARAMETER, "type", info::type_named_doc(m, n)))
        .collect();
    for (d, def) in m.defs.iter().enumerate() {
        if def.kind == DefKind::Enum {
            out.push(plain(&def.name, kind::ENUM, "enum", info::def_markdown(m, d)));
        }
    }
    out
}

/// A declared label as a completion.
fn label_item(m: &Model, d: usize) -> CompletionItem {
    let def = &m.defs[d];
    let detail = format!("label: {}", m.type_name(def.ty));
    plain(&def.name, kind::VALUE, detail, info::def_markdown(m, d))
}

/// The declared labels whose type passes `fits`, in declaration order.
fn labels_fitting(m: &Model, fits: impl Fn(Type) -> bool) -> Vec<CompletionItem> {
    m.defs
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == DefKind::Label && matches!(d.ty, T::V(t) if fits(t)))
        .map(|(d, _)| label_item(m, d))
        .collect()
}

/// The set labels: what a loop, `count`, `first` and `pick` visit.
fn labels(m: &Model) -> Vec<CompletionItem> {
    labels_fitting(m, |t| matches!(t, Type::Set(k) if k.member().is_some()))
}

/// Whether a label of type `t` (one thing or a set) holds things of `h`.
fn member_fits(t: Type, h: Handle) -> bool {
    let member = match t {
        Type::H(m, _) => Some(m),
        Type::Set(k) => k.member(),
        _ => None,
    };
    member.is_some_and(|m| m.fits(h))
}

/// The type the argument at code index `i` is wanted as: an API entry's
/// front-end parameter, or a `fn`'s declared one.
fn wanted(m: &Model, i: usize) -> Option<Type> {
    let (ni, commas) = info::enclosing_call(m, i)?;
    let name = m.text(&m.code[ni]);
    if let Some(d) = m.top_kind(name, DefKind::Fn) {
        let p = m.children(d, DefKind::Param).nth(commas as usize)?;
        return match m.defs[p].ty {
            T::V(t) => Some(t),
            _ => None,
        };
    }
    let e = caps::entry_named(name)?;
    sig::of(e)?.args.get(commas as usize).copied()
}

/// Everything an expression may name, ranked by `want`: labels of that
/// type, then locals, globals and options of it, then values that may be
/// `none` (their detail says so), then set labels to index with `[`, then
/// the rest.
fn ranked(m: &Model, off: usize, want: Type) -> Vec<CompletionItem> {
    let mut tiers: [Vec<CompletionItem>; 4] = Default::default();
    let mut taken: Vec<String> = Vec::new();
    let mut late: Vec<CompletionItem> = Vec::new();
    let maybe = |mut c: CompletionItem| {
        c.detail = format!("{}: may be none", c.detail);
        c
    };
    for (d, def) in m.defs.iter().enumerate() {
        if def.kind != DefKind::Label {
            continue;
        }
        let T::V(t) = def.ty else { continue };
        let tier = match t {
            _ if t.fits(want) => Some(0),
            Type::H(..) if t.fits(want.optional()) => Some(2),
            Type::Set(k) if want.handle().is_some() => {
                let indexed = m
                    .checked
                    .label(&def.name)
                    .is_some_and(|(_, l)| l.def.numbered() || l.def.per_team());
                let fits = k.member().is_some_and(|h| Type::H(h, false).fits(want.unwrapped()));
                (indexed && fits).then_some(3)
            }
            _ => None,
        };
        let c = label_item(m, d);
        match tier {
            Some(2) => tiers[2].push(maybe(c)),
            Some(3) => {
                let mut c = c;
                c.insert_text = format!("{}[", def.name);
                tiers[3].push(c);
            }
            Some(k) => tiers[k].push(c),
            None => late.push(c),
        }
        taken.push(def.name.clone());
    }
    let mut values: Vec<usize> = m.visible_locals(off);
    values.extend(
        m.defs
            .iter()
            .enumerate()
            .filter(|(_, d)| matches!(d.kind, DefKind::Var(Scope::Global) | DefKind::Option))
            .map(|(i, _)| i),
    );
    for d in values {
        let def = &m.defs[d];
        let T::V(t) = def.ty else { continue };
        if taken.contains(&def.name) {
            continue;
        }
        let c = plain(&def.name, kind::VARIABLE, m.type_name(def.ty), info::def_markdown(m, d));
        if t.fits(want) {
            tiers[1].push(c);
        } else if t.fits(want.optional()) {
            tiers[2].push(maybe(c));
        } else {
            continue;
        }
        taken.push(def.name.clone());
    }
    let mut out: Vec<CompletionItem> = tiers.into_iter().flatten().collect();
    out.extend(expression(m, off).into_iter().filter(|c| !taken.contains(&c.label)));
    out.extend(late);
    out
}

/// The kinds a label declares, after `label <name>:`.
fn label_kinds() -> Vec<CompletionItem> {
    [
        ("zone", "One zone: a trigger volume or a flag."),
        ("zones", "A set of zones."),
        ("object", "One object."),
        ("objects", "A set of objects."),
        ("zone?", "One zone, or none on a map that has none."),
        ("object?", "One object, or none on a map that has none."),
        ("spawns", "Spawn areas, passed to spawn_areas."),
    ]
    .iter()
    .map(|(k, doc)| plain(k, kind::TYPE_PARAMETER, "label kind", *doc))
    .collect()
}

/// What may follow a label's kind: requirements for a set, then its map
/// text.
fn requirements(set: bool) -> Vec<CompletionItem> {
    let doc = |w: &str| info::keyword_doc(w).unwrap_or("").to_string();
    let mut out = Vec::new();
    if set {
        out.push(plain("numbered", kind::KEYWORD, "requirement", doc("numbered")));
        out.push(item("numbered a..b", kind::SNIPPET, "requirement", doc("numbered"), "numbered ${1:0}..${2:1}", SNIPPET));
        out.push(item("at least", kind::SNIPPET, "requirement", doc("at"), "at least ${1:1}", SNIPPET));
        out.push(item("at most", kind::SNIPPET, "requirement", doc("at"), "at most ${1:16}", SNIPPET));
        out.push(item("exactly", kind::SNIPPET, "requirement", doc("exactly"), "exactly ${1:1}", SNIPPET));
        out.push(plain("per team", kind::KEYWORD, "requirement", doc("per")));
    }
    out.push(item(
        "= \"text\"",
        kind::SNIPPET,
        "map text",
        "The map's label text, when it is not the name.",
        "= \"${1:text}\"",
        SNIPPET,
    ));
    out
}

/// The built-in flag labels as whole declarations, at the name of a new
/// `label` line.
fn flag_labels() -> Vec<CompletionItem> {
    FLAG_LABELS
        .iter()
        .map(|(text, decl)| {
            item(
                format!("label {text}: {decl}"),
                kind::SNIPPET,
                "a netgame flag's label",
                format!("The label every map gives its netgame flags of this type: `{text}`."),
                format!("{text}: {decl}"),
                PLAIN_TEXT,
            )
        })
        .collect()
}

fn palettes(m: &Model, quoted: bool) -> Vec<CompletionItem> {
    m.defs
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == DefKind::Palette)
        .map(|(d, def)| {
            let insert = if quoted {
                format!("\"{}\"", def.name)
            } else {
                def.name.clone()
            };
            item(&def.name, kind::VALUE, "palette", info::def_markdown(m, d), insert, PLAIN_TEXT)
        })
        .collect()
}

fn sets() -> Vec<CompletionItem> {
    let doc = |w: &str| info::keyword_doc(w).unwrap_or("").to_string();
    vec![
        plain("players", kind::KEYWORD, "set", doc("players")),
        plain("teams", kind::KEYWORD, "set", doc("teams")),
        plain("objectives", kind::KEYWORD, "set", doc("objectives")),
        item(
            "pick",
            kind::FUNCTION,
            "set",
            LANG[2].doc,
            "pick(${1:players}, ${2:1})",
            SNIPPET,
        ),
    ]
}

/// What a declaration's default or initial value may be.
fn constants(m: &Model) -> Vec<CompletionItem> {
    let mut out = vec![
        plain("true", kind::KEYWORD, "bool", ""),
        plain("false", kind::KEYWORD, "bool", ""),
        plain("none", kind::KEYWORD, "", info::keyword_doc("none").unwrap_or("")),
        item("team", kind::KEYWORD, "team", info::keyword_doc("team").unwrap_or(""), "team ${1:0}", SNIPPET),
    ];
    for (d, def) in m.defs.iter().enumerate() {
        if def.kind == DefKind::Member
            && let Some(p) = def.parent
        {
            let label = format!("{}.{}", m.defs[p].name, def.name);
            out.push(item(
                &label,
                kind::ENUM_MEMBER,
                &m.defs[p].name,
                info::def_markdown(m, d),
                &label,
                PLAIN_TEXT,
            ));
        }
    }
    out
}

fn top_level(m: &Model) -> Vec<CompletionItem> {
    let carriers: Vec<&str> = CARRIERS.iter().map(|(n, _)| *n).collect();
    let carrier = format!("carrier ${{1|{}|}}", carriers.join(","));
    let snippets: Vec<(&str, String, &str)> = vec![
        ("rules", "rules \"${1:Name}\" ${2:1}".to_string(), "rules"),
        ("carrier", carrier, "carrier"),
        ("teams", "teams ${1:2}".to_string(), "teams"),
        ("list", "list ${1|tweakers|}".to_string(), "list"),
        ("requires", "requires ${1:capability}".to_string(), "requires"),
        (
            "label",
            "label ${1:name}: ${2|zone,zones,object,objects,spawns|}".to_string(),
            "label",
        ),
        (
            "option",
            "option ${1:name}: int ${2:1}..${3:5} = ${4:1} \"${5:Label}\" \"${6:Description}\"".to_string(),
            "option",
        ),
        (
            "option (bool)",
            "option ${1:name}: bool = ${2:true} \"${3:Label}\" \"${4:Description}\"".to_string(),
            "option",
        ),
        (
            "palette",
            "palette \"${1:name}\" = [\"${2:objects/...}\"]".to_string(),
            "palette",
        ),
        ("enum", "enum ${1:Name} { ${2:a}, ${3:b} }".to_string(), "enum"),
        ("global var", "global var ${1:name}: ${2:int} = ${3:0}".to_string(), "global"),
        ("player var", "player var ${1:name}: ${2:int} = ${3:0}".to_string(), "player"),
        ("team var", "team var ${1:name}: ${2:int} = ${3:0}".to_string(), "team"),
        ("object var", "object var ${1:name}: ${2:int} = ${3:0}".to_string(), "object"),
        (
            "table",
            "table ${1:name}: (${2:key}: ${3:int}, ${4:value}: ${5:int}) = [\n\t(${6:0}, ${7:0}),\n]".to_string(),
            "table",
        ),
        ("fn", "fn ${1:name}(${2:p}: ${3:player}) {\n\t$0\n}".to_string(), "fn"),
    ];
    let mut out: Vec<CompletionItem> = snippets
        .into_iter()
        .map(|(label, body, word)| {
            item(
                label,
                kind::SNIPPET,
                "declaration",
                info::keyword_doc(word).unwrap_or(""),
                body,
                SNIPPET,
            )
        })
        .collect();
    out.extend(events(m, "on "));
    out
}

fn statements() -> Vec<CompletionItem> {
    let doc = |w: &str| info::keyword_doc(w).unwrap_or("").to_string();
    vec![
        item("let", kind::SNIPPET, "statement", doc("let"), "let ${1:name} = $0", SNIPPET),
        item("if", kind::SNIPPET, "statement", doc("if"), "if ${1:cond} {\n\t$0\n}", SNIPPET),
        item(
            "for each",
            kind::SNIPPET,
            "statement",
            doc("for"),
            "for each ${1|players,teams,objectives|} ${2:x} {\n\t$0\n}",
            SNIPPET,
        ),
    ]
}

/// Everything an expression may name at `off`.
fn expression(m: &Model, off: usize) -> Vec<CompletionItem> {
    let mut out = Vec::new();
    for d in m.visible_locals(off) {
        let def = &m.defs[d];
        out.push(plain(&def.name, kind::VARIABLE, m.type_name(def.ty), info::def_markdown(m, d)));
    }
    for (d, def) in m.defs.iter().enumerate() {
        let (k, detail) = match def.kind {
            DefKind::Var(Scope::Global) => (kind::VARIABLE, format!("global var: {}", m.type_name(def.ty))),
            DefKind::Option => (kind::CONSTANT, format!("option: {}", m.type_name(def.ty))),
            DefKind::Table => (kind::STRUCT, "table".to_string()),
            DefKind::Enum => (kind::ENUM, "enum".to_string()),
            DefKind::Fn => {
                let (label, _) = info::fn_signature(m, d);
                let params: Vec<String> = m.children(d, DefKind::Param).map(|p| m.defs[p].name.clone()).collect();
                out.push(item(
                    &def.name,
                    kind::FUNCTION,
                    format!("fn {label}"),
                    info::def_markdown(m, d),
                    format!("{}({})", def.name, placeholders(&params)),
                    SNIPPET,
                ));
                continue;
            }
            _ => continue,
        };
        out.push(plain(&def.name, k, detail, info::def_markdown(m, d)));
    }
    // The team and everyone forms of `message` are reached through it.
    for e in ENTRIES.iter().filter(|e| !caps::is_form(e)) {
        let params: Vec<String> = if e.id == caps::id::MESSAGE {
            vec!["to".into(), "text".into()]
        } else {
            info::entry_parts(e).0
        };
        out.push(item(
            e.name,
            kind::FUNCTION,
            info::entry_signature(e).0,
            info::entry_doc(e),
            format!("{}({})", e.name, placeholders(&params)),
            SNIPPET,
        ));
    }
    for l in LANG {
        out.push(item(
            l.name,
            kind::FUNCTION,
            info::lang_signature(l).0,
            l.doc,
            l.snippet,
            SNIPPET,
        ));
    }
    for w in ["true", "false", "none", "not", "and", "or", "team", "text", "is"] {
        out.push(plain(w, kind::KEYWORD, "", info::keyword_doc(w).unwrap_or("")));
    }
    out.push(plain(
        "variant",
        kind::MODULE,
        "the variant record",
        info::keyword_doc("variant").unwrap_or(""),
    ));
    out
}
