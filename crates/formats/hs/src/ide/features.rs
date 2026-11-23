//! The requests over one [`Model`]: completion, hover, signature help,
//! navigation, rename, the outline, folding, semantic tokens and quick
//! fixes.

use super::context::{BuiltinInfo, Context};
use super::model::{DefItem, Model, Occ, Role, is_number};
use super::syntax::{List, Node, Tok};
use super::{
    CodeAction, CompletionItem, FoldingRange, Highlight, Hover, Markup, Position, PrepareRename, Range,
    Signature, SignatureHelp, Symbol, TextEdit, Token, TokenKind, completion_kind as ck, highlight_kind,
};

/// The most completions one request answers.
const MAX_COMPLETIONS: usize = 300;

fn word_char(b: u8) -> bool {
    !(b.is_ascii_whitespace() || matches!(b, b'(' | b')' | b'"' | b';'))
}

/// Where `off` is: inside a string or comment, or the atom being typed
/// (its start), the list it is in, and its index there.
struct Place<'t> {
    word_start: usize,
    /// The innermost list holding the word, with the chain of lists
    /// from the top (outermost first).
    chain: Vec<&'t List>,
    index: usize,
}

fn in_string_or_comment(m: &Model, off: usize) -> bool {
    m.tree.lexemes.iter().any(|lx| {
        matches!(lx.kind, Tok::Str | Tok::Comment) && lx.start < off && (off < lx.end || (off == lx.end && lx.kind == Tok::Comment))
    })
}

fn place<'t>(m: &'t Model, off: usize) -> Place<'t> {
    let b = m.src.as_bytes();
    let mut word_start = off.min(b.len());
    while word_start > 0 && word_char(b[word_start - 1]) {
        word_start -= 1;
    }
    let mut chain = Vec::new();
    let mut items: &'t [Node] = &m.tree.items;
    loop {
        let inner = items.iter().find_map(|n| match n {
            Node::List(l) if l.open < word_start && l.close.is_none_or(|c| word_start <= c) => Some(l),
            _ => None,
        });
        match inner {
            Some(l) => {
                chain.push(l);
                items = &l.items;
            }
            None => break,
        }
    }
    let index = items.iter().filter(|n| n.span().1 <= word_start).count();
    Place { word_start, chain, index }
}

fn item(label: impl Into<String>, kind: u8, detail: Option<String>, documentation: Option<String>, rank: u8) -> CompletionItem {
    let label = label.into();
    CompletionItem {
        sort_text: format!("{rank}{label}"),
        label,
        kind,
        detail,
        documentation: documentation.map(Markup::markdown),
    }
}

fn builtin_doc(b: &BuiltinInfo) -> String {
    let mut out = format!("```haloscript\n{}\n```\nReturns `{}`.", b.call_form(), b.returns);
    if b.own_arguments {
        out.push_str(" Reads its own arguments: the parameters shown are not its whole signature.");
    }
    if let Some(d) = b.doc.as_deref().filter(|d| !d.is_empty()) {
        out.push_str("\n\n");
        out.push_str(d);
    }
    if let Some(u) = b.usage.as_deref().filter(|u| !u.is_empty()) {
        out.push_str(&format!("\n\nUsage: `{u}`"));
    }
    match b.from.as_deref() {
        Some("map") => out.push_str("\n\n*Named by a call in this map's scripts.*"),
        Some("doc") => out.push_str("\n\n*Named by the script documentation: its types match.*"),
        _ => out.push_str("\n\n*Unnamed: nothing names this builtin.*"),
    }
    out
}

/// Completions at `pos`.
pub fn complete(src: &str, pos: Position, ctx: &Context) -> Vec<CompletionItem> {
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    if in_string_or_comment(&m, off) {
        return Vec::new();
    }
    let p = place(&m, off);
    let prefix = m.src[p.word_start..off].to_lowercase();
    let mut out: Vec<CompletionItem> = Vec::new();
    match p.chain.len() {
        // Top level: nothing but definitions.
        0 => {}
        1 => {
            let def = p.chain[0];
            let head = def.items.first().and_then(|h| h.atom(src));
            match (p.index, head) {
                (0, _) => {
                    out.push(item("global", ck::KEYWORD, Some("(global <type> <name> <value>)".into()), None, 0));
                    out.push(item("script", ck::KEYWORD, Some("(script <kind> <name> <expression>...)".into()), None, 0));
                }
                (1, Some("global")) => types(ctx, &mut out),
                (1, Some("script")) => {
                    for k in &ctx.script_kinds {
                        out.push(item(k, ck::KEYWORD, Some("script kind".into()), None, 0));
                    }
                }
                (2, Some("script"))
                    if def.items.get(1).and_then(|k| k.atom(src)).is_some_and(|k| ctx.typed_kinds.iter().any(|t| t == k)) =>
                {
                    types(ctx, &mut out)
                }
                (i, Some(_)) => {
                    let body_from = m.def_at(def.open).map_or(usize::MAX, |d| m.defs[d].body_from);
                    if i >= body_from {
                        values(&m, None, &mut out);
                    }
                }
                _ => {}
            }
        }
        _ => {
            let list = *p.chain.last().expect("a list");
            let parent_cond = p.chain.len() >= 2
                && p.chain[p.chain.len() - 2].items.first().and_then(|h| h.atom(src)) == Some("cond");
            if p.index == 0 && !parent_cond {
                heads(&m, &prefix, &mut out);
            } else {
                let param = list
                    .items
                    .first()
                    .and_then(|h| h.atom(src))
                    .and_then(|h| m.builtin(h))
                    .filter(|(_, b)| !b.own_arguments)
                    .and_then(|(_, b)| b.parameters.get(p.index.wrapping_sub(1)).cloned());
                values(&m, param.as_deref(), &mut out);
                if parent_cond && p.index == 0 {
                    heads(&m, &prefix, &mut out);
                }
            }
        }
    }
    let mut out: Vec<CompletionItem> = out
        .into_iter()
        .filter(|c| prefix.is_empty() || c.label.to_lowercase().contains(&prefix))
        .collect();
    // Names that start with what is typed first, then those that hold it.
    out.sort_by_key(|c| (!c.label.to_lowercase().starts_with(&prefix), c.sort_text.clone()));
    out.dedup_by(|a, b| a.label == b.label);
    out.truncate(MAX_COMPLETIONS);
    out
}

fn types(ctx: &Context, out: &mut Vec<CompletionItem>) {
    for t in &ctx.types {
        out.push(item(t, ck::TYPE_PARAMETER, Some("value type".into()), None, 0));
    }
}

/// What can name a call: every named builtin (an unnamed one only when
/// `#` is typed) and every script.
fn heads(m: &Model, prefix: &str, out: &mut Vec<CompletionItem>) {
    for d in m.defs.iter().filter(|d| d.item == DefItem::Script) {
        out.push(item(&d.name, ck::METHOD, Some(format!("(script {} {})", d.kind, d.name)), None, 0));
    }
    for b in &m.ctx.builtins {
        if b.name.is_none() && !prefix.starts_with('#') {
            continue;
        }
        out.push(item(b.label(), ck::FUNCTION, Some(format!("{} → {}", b.call_form(), b.returns)), Some(builtin_doc(b)), 1));
    }
}

/// What can stand as a value of type `param` (any, when `None`): the
/// scenario's names of that type, the globals of that type, scripts for
/// a script parameter, and the constants.
fn values(m: &Model, param: Option<&str>, out: &mut Vec<CompletionItem>) {
    let fits = |t: &str| param.is_none_or(|p| p == t);
    if let Some(p) = param
        && let Some(names) = m.ctx.names_for(p)
    {
        for n in names {
            out.push(item(n, ck::ENUM_MEMBER, Some(p.to_owned()), None, 0));
        }
    }
    for d in m.defs.iter().filter(|d| d.item == DefItem::Global && fits(&d.type_name)) {
        out.push(item(&d.name, ck::VARIABLE, Some(format!("(global {} {})", d.type_name, d.name)), None, 1));
    }
    for g in m.ctx.external_globals.iter().filter(|g| fits(&g.type_name)) {
        out.push(item(&g.name, ck::VARIABLE, Some(format!("game global: {}", g.type_name)), None, 2));
    }
    if param.is_none_or(|p| p == "script") {
        for d in m.defs.iter().filter(|d| d.item == DefItem::Script) {
            out.push(item(&d.name, ck::METHOD, Some(format!("(script {} {})", d.kind, d.name)), None, 3));
        }
    }
    if param.is_none_or(|p| p == "boolean") {
        for c in ["true", "false"] {
            out.push(item(c, ck::CONSTANT, None, None, 4));
        }
    }
}

/// What the name under `pos` is, in a sentence of Markdown.
pub fn hover(src: &str, pos: Position, ctx: &Context) -> Option<Hover> {
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    let o = m.occ_at(off)?;
    let text = &src[o.start..o.end];
    let value = match &o.role {
        Role::CallBuiltin(i) => builtin_doc(&ctx.builtins[*i]),
        Role::CallScript(d) | Role::ScriptRef(d) | Role::DefName(d) => {
            let d = &m.defs[*d];
            let header = match d.item {
                DefItem::Script if ctx.typed_kinds.contains(&d.kind) => format!("(script {} {} {})", d.kind, d.type_name, d.name),
                DefItem::Script => format!("(script {} {})", d.kind, d.name),
                DefItem::Global => format!("(global {} {})", d.type_name, d.name),
            };
            let uses = references_of(&m, o).len().saturating_sub(1);
            format!("```haloscript\n{header}\n```\nNamed {uses} time{} in this file.", if uses == 1 { "" } else { "s" })
        }
        Role::GlobalRef(d) => {
            let d = &m.defs[*d];
            format!("```haloscript\n(global {} {})\n```", d.type_name, d.name)
        }
        Role::ExtGlobal(i) => format!("`{text}`: a game global, `{}`.", ctx.external_globals[*i].type_name),
        Role::Name(t) => {
            let what = if ctx.is_object_type(t) {
                "an object name".to_owned()
            } else if t == "trigger_volume" {
                "a trigger volume".to_owned()
            } else {
                format!("a `{t}`")
            };
            let map = if ctx.map.is_empty() { "the map" } else { ctx.map.as_str() };
            format!("`{text}`: {what} in {map}.")
        }
        Role::Type => format!("`{text}`: a value type."),
        Role::Kind => format!("`{text}`: a script kind."),
        Role::Keyword => match text {
            "global" => "`(global <type> <name> <value>)`: a scenario global.".to_owned(),
            _ => "`(script <kind> <name> <expression>...)`: a scenario script.".to_owned(),
        },
        Role::CallUnknown => format!("No builtin or script is named `{text}`."),
        Role::Number | Role::Constant | Role::Symbol => match &o.param {
            Some(p) => format!("`{text}` as a `{p}`."),
            None => return None,
        },
    };
    Some(Hover {
        contents: Markup::markdown(value),
        range: m.lines.range(o.start, o.end),
    })
}

/// The builtin call around `pos` and the argument it is at.
pub fn signature_help(src: &str, pos: Position, ctx: &Context) -> Option<SignatureHelp> {
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    if in_string_or_comment(&m, off) {
        return None;
    }
    let p = place(&m, off);
    // The innermost list that is a call of a builtin.
    let (list, index) = {
        let list = *p.chain.last()?;
        (list, p.index)
    };
    let head = list.items.first()?.atom(src)?;
    let (_, b) = m.builtin(head)?;
    if index == 0 {
        return None;
    }
    let mut label = format!("({}", b.label());
    let mut parameters = Vec::new();
    for t in &b.parameters {
        label.push(' ');
        let start = label.encode_utf16().count() as u32;
        label.push_str(&format!("<{t}>"));
        parameters.push([start, label.encode_utf16().count() as u32]);
    }
    label.push(')');
    Some(SignatureHelp {
        signature: Signature {
            label,
            documentation: Some(builtin_doc(b)),
            parameters,
        },
        active_parameter: (index - 1) as u32,
    })
}

/// Whether two atoms name the same thing.
fn same(m: &Model, a: &Occ, b: &Occ) -> bool {
    let script = |r: &Role| match r {
        Role::CallScript(d) | Role::ScriptRef(d) => Some(*d),
        Role::DefName(d) if m.defs[*d].item == DefItem::Script => Some(*d),
        _ => None,
    };
    let global = |r: &Role| match r {
        Role::GlobalRef(d) => Some(*d),
        Role::DefName(d) if m.defs[*d].item == DefItem::Global => Some(*d),
        _ => None,
    };
    if let Some(s) = script(&a.role) {
        return script(&b.role) == Some(s);
    }
    if let Some(g) = global(&a.role) {
        return global(&b.role) == Some(g);
    }
    let text = |o: &Occ| &m.src[o.start..o.end];
    match (&a.role, &b.role) {
        (Role::CallBuiltin(x), Role::CallBuiltin(y)) => x == y,
        (Role::ExtGlobal(x), Role::ExtGlobal(y)) => x == y,
        (Role::Name(_), Role::Name(_)) | (Role::CallUnknown, Role::CallUnknown) | (Role::Symbol, Role::Symbol) => {
            text(a) == text(b)
        }
        _ => false,
    }
}

/// Every atom naming what `o` names, the definition first.
fn references_of<'m>(m: &'m Model, o: &Occ) -> Vec<&'m Occ> {
    if matches!(o.role, Role::Keyword | Role::Kind | Role::Type | Role::Number | Role::Constant) {
        return Vec::new();
    }
    let mut out: Vec<&Occ> = m.occs.iter().filter(|x| same(m, o, x)).collect();
    out.sort_by_key(|x| (!matches!(x.role, Role::DefName(_)), x.start));
    out
}

/// Where the script or global under `pos` is defined.
pub fn definition(src: &str, pos: Position, ctx: &Context) -> Vec<Range> {
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    let Some(o) = m.occ_at(off) else {
        return Vec::new();
    };
    let d = match o.role {
        Role::CallScript(d) | Role::ScriptRef(d) | Role::GlobalRef(d) | Role::DefName(d) => d,
        _ => return Vec::new(),
    };
    let (s, e) = m.defs[d].name_span;
    vec![m.lines.range(s, e)]
}

/// Every place the name under `pos` is named; its definition first when
/// `declaration`.
pub fn references(src: &str, pos: Position, ctx: &Context, declaration: bool) -> Vec<Range> {
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    let Some(o) = m.occ_at(off) else {
        return Vec::new();
    };
    references_of(&m, o)
        .into_iter()
        .filter(|x| declaration || !matches!(x.role, Role::DefName(_)))
        .map(|x| m.lines.range(x.start, x.end))
        .collect()
}

/// The uses of the name under `pos`: its definition written, the rest read.
pub fn highlights(src: &str, pos: Position, ctx: &Context) -> Vec<Highlight> {
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    let Some(o) = m.occ_at(off) else {
        return Vec::new();
    };
    references_of(&m, o)
        .into_iter()
        .map(|x| Highlight {
            range: m.lines.range(x.start, x.end),
            kind: if matches!(x.role, Role::DefName(_)) { highlight_kind::WRITE } else { highlight_kind::READ },
        })
        .collect()
}

/// The script or global a rename at `pos` would rename, or why none.
pub fn prepare_rename(src: &str, pos: Position, ctx: &Context) -> Result<PrepareRename, String> {
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    let o = m.occ_at(off).ok_or("no name here")?;
    match o.role {
        Role::CallScript(d) | Role::ScriptRef(d) | Role::GlobalRef(d) | Role::DefName(d) => Ok(PrepareRename {
            range: m.lines.range(o.start, o.end),
            placeholder: m.defs[d].name.clone(),
        }),
        Role::CallBuiltin(_) => Err("a builtin's name is the executable's: it cannot be renamed".into()),
        Role::Name(_) => Err("a scenario name is renamed in Studio, where the object or volume is".into()),
        _ => Err("only a script or a global of this file can be renamed".into()),
    }
}

/// The edits renaming the script or global under `pos` to `to`.
pub fn rename(src: &str, pos: Position, ctx: &Context, to: &str) -> Result<Vec<TextEdit>, String> {
    prepare_rename(src, pos, ctx)?;
    if to.is_empty() || to.bytes().any(|b| !word_char(b)) || is_number(to) {
        return Err(format!("{to:?} is not a name: no spaces, parentheses, quotes or ;"));
    }
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    let o = m.occ_at(off).ok_or("no name here")?;
    if m.defs.iter().any(|d| d.name == to) || m.builtin(to).is_some() || matches!(to, "true" | "false" | "none" | "global" | "script") {
        return Err(format!("{to} is already a name here"));
    }
    Ok(references_of(&m, o)
        .into_iter()
        .map(|x| TextEdit {
            range: m.lines.range(x.start, x.end),
            new_text: to.to_owned(),
        })
        .collect())
}

/// The outline: every definition.
pub fn symbols(src: &str, ctx: &Context) -> Vec<Symbol> {
    let m = Model::new(src, ctx);
    m.defs
        .iter()
        .map(|d| Symbol {
            name: d.name.clone(),
            detail: match d.item {
                DefItem::Global => d.type_name.clone(),
                DefItem::Script => format!("{} {}", d.kind, d.type_name),
            },
            kind: match d.item {
                DefItem::Script => 12,
                DefItem::Global => 13,
            },
            range: m.lines.range(d.span.0, d.span.1),
            selection_range: m.lines.range(d.name_span.0, d.name_span.1),
        })
        .collect()
}

/// Every list and block comment over more than one line.
pub fn folding_ranges(src: &str) -> Vec<FoldingRange> {
    let m = Model::new(src, &EMPTY);
    let mut out = Vec::new();
    fn walk(m: &Model, items: &[Node], out: &mut Vec<FoldingRange>) {
        for n in items {
            if let Node::List(l) = n {
                let (a, b) = (m.lines.position(l.open).line, m.lines.position(l.end.saturating_sub(1)).line);
                if b > a {
                    out.push(FoldingRange {
                        start_line: a,
                        end_line: b,
                        kind: None,
                    });
                }
                walk(m, &l.items, out);
            }
        }
    }
    walk(&m, &m.tree.items, &mut out);
    for lx in m.tree.lexemes.iter().filter(|l| l.kind == Tok::Comment) {
        let (a, b) = (m.lines.position(lx.start).line, m.lines.position(lx.end.saturating_sub(1)).line);
        if b > a {
            out.push(FoldingRange {
                start_line: a,
                end_line: b,
                kind: Some("comment"),
            });
        }
    }
    out.sort_by_key(|f| (f.start_line, std::cmp::Reverse(f.end_line)));
    out
}

static EMPTY: Context = Context {
    version: 0,
    map: String::new(),
    types: Vec::new(),
    script_kinds: Vec::new(),
    typed_kinds: Vec::new(),
    builtins: Vec::new(),
    external_globals: Vec::new(),
    object_types: Vec::new(),
    names: std::collections::BTreeMap::new(),
};

/// Every token's colour, one line at a time, in order.
pub fn semantic_tokens(src: &str, ctx: &Context) -> Vec<Token> {
    let m = Model::new(src, ctx);
    let mut spans: Vec<(usize, usize, TokenKind)> = Vec::new();
    for o in &m.occs {
        let kind = match &o.role {
            Role::Keyword | Role::Kind => TokenKind::Keyword,
            Role::Type => TokenKind::Type,
            Role::CallBuiltin(i) if ctx.builtins[*i].own_arguments => TokenKind::Macro,
            Role::CallBuiltin(_) => TokenKind::Function,
            Role::CallScript(_) | Role::ScriptRef(_) => TokenKind::Method,
            Role::DefName(d) => match m.defs[*d].item {
                DefItem::Script => TokenKind::Method,
                DefItem::Global => TokenKind::Variable,
            },
            Role::GlobalRef(_) | Role::ExtGlobal(_) => TokenKind::Variable,
            Role::Name(_) => TokenKind::EnumMember,
            Role::Number => TokenKind::Number,
            Role::Constant => TokenKind::Keyword,
            Role::CallUnknown | Role::Symbol => continue,
        };
        spans.push((o.start, o.end, kind));
    }
    for lx in &m.tree.lexemes {
        match lx.kind {
            Tok::Str => spans.push((lx.start, lx.end, TokenKind::String)),
            Tok::Comment => spans.push((lx.start, lx.end, TokenKind::Comment)),
            _ => {}
        }
    }
    spans.sort_by_key(|s| s.0);
    let mut out = Vec::new();
    for (start, end, kind) in spans {
        // One token per line: a block comment is split at its line ends.
        let mut at = start;
        while at < end {
            let line_end = src[at..end].find('\n').map_or(end, |i| at + i);
            let a = m.lines.position(at);
            let len = src[at..line_end].encode_utf16().count() as u32;
            if len > 0 {
                out.push(Token {
                    line: a.line,
                    start: a.character,
                    len,
                    kind,
                });
            }
            at = line_end + 1;
        }
    }
    out
}

/// Quick fixes for the problems in `range`: a misspelt call becomes the
/// name it is nearest to.
pub fn actions(src: &str, range: Range, ctx: &Context) -> Vec<CodeAction> {
    let m = Model::new(src, ctx);
    let mut out = Vec::new();
    for d in &m.diagnostics {
        if d.code != "call.unknown" || d.range.end < range.start || range.end < d.range.start {
            continue;
        }
        let (s, e) = (m.lines.offset(d.range.start), m.lines.offset(d.range.end));
        let Some(near) = m.nearest(&src[s..e]) else {
            continue;
        };
        out.push(CodeAction {
            title: format!("Change to `{near}`"),
            kind: "quickfix",
            diagnostic: d.clone(),
            edits: vec![TextEdit {
                range: d.range,
                new_text: near,
            }],
        });
    }
    out
}

/// What the name under `pos` is to Studio's `script.uses`: `script`,
/// `global`, `object` or `volume`, and the name. Off a name, the
/// definition holding `pos` (as a script or global).
pub fn name_at(src: &str, pos: Position, ctx: &Context) -> Option<(&'static str, String)> {
    let m = Model::new(src, ctx);
    let off = m.lines.offset(pos);
    if let Some(o) = m.occ_at(off) {
        let text = src[o.start..o.end].to_owned();
        let kind = match &o.role {
            Role::CallScript(_) | Role::ScriptRef(_) => Some("script"),
            Role::GlobalRef(_) => Some("global"),
            Role::DefName(d) => Some(m.defs[*d].item.name()),
            Role::Name(t) if ctx.is_object_type(t) || t == "object_name" => Some("object"),
            Role::Name(t) if t == "trigger_volume" => Some("volume"),
            _ => None,
        };
        if let Some(k) = kind {
            return Some((k, text));
        }
    }
    let d = &m.defs[m.def_at(off)?];
    Some((d.item.name(), d.name.clone()))
}
