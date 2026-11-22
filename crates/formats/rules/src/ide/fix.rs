//! Fixes, code actions and rename: text edits the editor applies. Each
//! fix answers one diagnostic code of docs/game/rules-language-types.md,
//! "Diagnostics and the editor", and is built from the source's tokens,
//! the tree and what the checker said, never from a type of its own.

use super::info::{self, ref_near};
use super::model::{DefKind, Model, item_pos};
use super::scan::K;
use super::{CodeAction, Fix, Range, TextEdit};
use crate::caps;
use crate::ir::Event;
use crate::lang::ast::*;
use crate::lang::parse::KEYWORDS;
use crate::lang::sig;
use crate::lang::types::{Handle, Type};
use crate::lang::{self, Pos};

fn edit(m: &Model, s: usize, e: usize, text: impl Into<String>) -> TextEdit {
    TextEdit {
        range: m.li.range(s, e),
        new_text: text.into(),
    }
}

fn fix(title: impl Into<String>, edits: Vec<TextEdit>) -> Fix {
    Fix {
        title: title.into(),
        edits,
    }
}

fn meets(a: Range, b: Range) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// The fixes of every diagnostic whose range meets `range`.
pub(crate) fn actions(m: &Model, range: Range) -> Vec<CodeAction> {
    let mut out = Vec::new();
    for mut d in info::diagnostics(m) {
        if d.fixes.is_empty() || !meets(d.range, range) {
            continue;
        }
        let fixes = std::mem::take(&mut d.fixes);
        for f in fixes {
            out.push(CodeAction {
                title: f.title,
                kind: "quickfix",
                diagnostic: d.clone(),
                edits: f.edits,
            });
        }
    }
    out
}

/// The fixes of one of the checker's diagnostics.
pub(crate) fn fixes(m: &Model, d: &lang::Diagnostic) -> Vec<Fix> {
    let at = m.byte(d.pos);
    let msg = d.message.as_str();
    let first = msg.split(' ').next().unwrap_or("");
    let mut out = Vec::new();
    match d.code {
        "label.unknown" => label_unknown(m, msg, at, &mut out),
        "label.set-as-one" => out.extend(wrap_for_each(m, at)),
        "label.one-as-set" => change_kind(m, first, &[("object", "objects"), ("zone", "zones")], &mut out),
        "label.kind" => change_kind(m, first, &[("object", "zone"), ("objects", "zones")], &mut out),
        "label.index" if msg.contains("is not numbered") => add_numbered(m, first, &mut out),
        "label.unused" => remove_label(m, at, &mut out),
        "null.use" | "null.arg" => null_fixes(m, d.pos, &mut out),
        "null.var" if msg.contains("starts as none") => {
            let (_, e) = m.tok_at(d.pos);
            out.push(fix("Add `?`", vec![edit(m, e, e, "?")]));
        }
        "null.never" => remove_check(m, at, &mut out),
        "tag.group" if msg.contains("Declare the column as weapon") => column_weapon(m, msg, &mut out),
        "cap.missing" => add_requires(m, msg, &mut out),
        "cap.unused" => remove_requires(m, at, &mut out),
        _ => {}
    }
    out
}

// ---------- labels ----------

/// The declaration of the label named `name`.
fn decl<'s>(m: &Model<'s>, name: &str) -> Option<&'s LabelDecl> {
    m.labels.get(name).and_then(|&d| m.label_decl(d))
}

/// `no label named barier.`: change it to the close one the message
/// names, or declare it.
fn label_unknown(m: &Model, msg: &str, at: usize, out: &mut Vec<Fix>) {
    let Some(rest) = msg.strip_prefix("no label named ") else { return };
    let name = rest.split(['.', ' ']).next().unwrap_or("");
    if name.is_empty() {
        return;
    }
    // The name's token: at the diagnostic, or after it on its line (a
    // handler's label is reported at its `on`).
    let line = m.li.line_of(at);
    let Some(i) = (m.ci(at)..m.code.len())
        .take_while(|&j| m.li.line_of(m.code[j].s) == line)
        .find(|&j| m.code[j].k == K::Ident && m.text(&m.code[j]) == name)
    else {
        return;
    };
    let tok = m.code[i];
    if let Some(k) = msg.find("Did you mean ") {
        let close = msg[k + "Did you mean ".len()..].trim_end_matches('?');
        out.push(fix(format!("Change to `{close}`"), vec![edit(m, tok.s, tok.e, close)]));
    }
    let kind = declared_kind(m, i);
    let (off, before) = declaration_place(m);
    let text = format!("{before}label {name}: {kind}\n");
    out.push(fix(format!("Declare label `{name}`"), vec![edit(m, off, off, text)]));
}

/// The kind a new label takes from where the name at code index `i` is
/// used: a set after `is`, `each`, `count(`, `first(` and `pick(` and in
/// a handler's place, else one thing of the type the call wants.
fn declared_kind(m: &Model, i: usize) -> &'static str {
    let prev = i.checked_sub(1);
    let ptext = prev.map_or("", |j| m.text(&m.code[j]));
    if matches!(ptext, "is" | "each") {
        return "objects";
    }
    let set_call = prev
        .and_then(|j| j.checked_sub(1))
        .is_some_and(|k| matches!(m.text(&m.code[k]), "count" | "first" | "pick"));
    if ptext == "(" && set_call {
        return "objects";
    }
    if let Some((ni, commas)) = info::enclosing_call(m, i) {
        let name = m.text(&m.code[ni]);
        if ni > 0 && m.is_word(ni - 1, "on") {
            return match Event::named(name).and_then(sig::label_arg) {
                Some((_, Handle::Zone)) => "zones",
                _ => "objects",
            };
        }
        let want = caps::entry_named(name)
            .and_then(sig::of)
            .and_then(|s| s.args.get(commas as usize).copied());
        if let Some(Type::H(h, _)) = want {
            return if h == Handle::Zone { "zone" } else { "object" };
        }
    }
    "object"
}

/// Where a new declaration goes: after the last `label` line, else before
/// the first item, else at the end; with a line break first when the file
/// does not end with one.
fn declaration_place(m: &Model) -> (usize, &'static str) {
    let script = m.script;
    let last_label = script.items.iter().rposition(|it| matches!(it, Item::Label(_)));
    if let Some(i) = last_label
        && let Some(&(_, end)) = m.items.get(i)
    {
        return line_after(m, end);
    }
    match script.items.first() {
        Some(it) => (m.li.line_start(m.li.line_of(m.byte(item_pos(it)))), ""),
        None => line_after(m, m.src.len()),
    }
}

/// The start of the line after the one holding `off`.
fn line_after(m: &Model, off: usize) -> (usize, &'static str) {
    let s = m.li.line_start(m.li.line_of(off) + 1);
    if s >= m.src.len() && !m.src.is_empty() && !m.src.ends_with('\n') {
        (m.src.len(), "\n")
    } else {
        (s, "")
    }
}

/// `label core: object` to `objects`, `label pad: object` to `zone`.
fn change_kind(m: &Model, name: &str, map: &[(&str, &str)], out: &mut Vec<Fix>) {
    let Some(l) = decl(m, name) else { return };
    let Some(&(_, to)) = map.iter().find(|(from, _)| *from == l.kind.name) else {
        return;
    };
    let (s, mut e) = m.tok_at(l.kind.pos);
    // A set has no `?`.
    if to.ends_with('s') && l.kind.optional {
        let i = m.ci(e);
        if m.is_punct(i, "?") {
            e = m.code[i].e;
        }
    }
    out.push(fix(format!("Change the declaration to `{to}`"), vec![edit(m, s, e, to)]));
}

fn add_numbered(m: &Model, name: &str, out: &mut Vec<Fix>) {
    let Some(l) = decl(m, name) else { return };
    let (_, e) = m.tok_at(l.kind.pos);
    out.push(fix("Add `numbered`", vec![edit(m, e, e, " numbered")]));
}

/// Removes the label declared at `at`, its whole lines.
fn remove_label(m: &Model, at: usize, out: &mut Vec<Fix>) {
    let script = m.script;
    let Some(i) = script
        .items
        .iter()
        .position(|it| matches!(it, Item::Label(l) if m.byte(l.pos) == at))
    else {
        return;
    };
    let Some(&(s, e)) = m.items.get(i) else { return };
    let from = m.li.line_start(m.li.line_of(s));
    let to = m.li.line_start(m.li.line_of(e) + 1);
    out.push(fix("Remove the declaration", vec![edit(m, from, to, "")]));
}

/// `delete(barrier)` to `for each barrier b { delete(b) }`, when the use
/// is a one-line statement.
fn wrap_for_each(m: &Model, at: usize) -> Option<Fix> {
    let i = m.token_at(at)?;
    let tok = m.code[i];
    let label = m.text(&tok);
    m.labels.get(label)?;
    let (s, e) = statement_line(m, at)?;
    let b = fresh(m, &m.src[s..e], label);
    let body = format!("{}{b}{}", &m.src[s..tok.s], &m.src[tok.e..e]);
    Some(fix(
        format!("Wrap in `for each {label} {b}`"),
        vec![edit(m, s, e, format!("for each {label} {b} {{ {body} }}"))],
    ))
}

// ---------- handles that may be none ----------

/// A use of a value that may be `none`: wrap the statement in a test
/// (`if x != none` for a let, parameter, argument or member; `if let v =
/// ...` for anything else), or put `x != none and` first in the `if`
/// whose condition holds it.
fn null_fixes(m: &Model, pos: Pos, out: &mut Vec<Fix>) {
    let at = m.byte(pos);
    let Some(e) = expr_at(m.script, pos) else { return };
    let (s, end) = (m.byte(leftmost(e).pos), expr_end(m, e));
    if end <= s {
        return;
    }
    let text = &m.src[s..end];
    let binding = matches!(e.kind, ExprKind::Name(_))
        && m.ref_at(s).is_some_and(|r| {
            matches!(
                m.defs[r.def].kind,
                DefKind::Local | DefKind::Arg | DefKind::Param | DefKind::Binder
            )
        });
    let line = statement_line(m, at).filter(|&(ls, le)| ls <= s && end <= le);
    if binding {
        if let Some(f) = guard_condition(m, at, text) {
            out.push(f);
        }
        if let Some((ls, le)) = line {
            let stmt = &m.src[ls..le];
            out.push(fix(
                format!("Wrap in `if {text} != none`"),
                vec![edit(m, ls, le, format!("if {text} != none {{ {stmt} }}"))],
            ));
        }
    } else if let Some((ls, le)) = line {
        let v = fresh(m, &m.src[ls..le], "v");
        let body = format!("{}{v}{}", &m.src[ls..s], &m.src[end..le]);
        out.push(fix(
            format!("Wrap in `if let {v} = {text}`"),
            vec![edit(m, ls, le, format!("if let {v} = {text} {{ {body} }}"))],
        ));
    }
}

/// `if <cond> {` on the line holding `at`, with the use in `cond`: `x !=
/// none and` put first, the rest in parentheses when it has an `or`.
fn guard_condition(m: &Model, at: usize, x: &str) -> Option<Fix> {
    let line = m.li.line_of(at);
    let (a, b) = (m.ci(m.li.line_start(line)), m.ci(m.li.line_end(line)));
    let iff = (a..b)
        .filter(|&j| m.code[j].s < at && m.is_word(j, "if") && !m.is_word(j + 1, "let"))
        .last()?;
    let open = (a..b).find(|&j| m.code[j].s > at && m.is_punct(j, "{"))?;
    if open <= iff + 1 {
        return None;
    }
    let (cs, ce) = (m.code[iff + 1].s, m.code[open - 1].e);
    let edits = if (iff + 1..open).any(|j| m.is_word(j, "or")) {
        vec![edit(m, cs, cs, format!("{x} != none and (")), edit(m, ce, ce, ")")]
    } else {
        vec![edit(m, cs, cs, format!("{x} != none and "))]
    };
    Some(fix(format!("Add `{x} != none and`"), edits))
}

/// `if p != none and p.alive` where `p` is never none: drop the test.
fn remove_check(m: &Model, at: usize, out: &mut Vec<Fix>) {
    let i = m.ci(at);
    if !(m.is_punct(i + 1, "!=") && m.is_word(i + 2, "none")) {
        return;
    }
    let e = if m.is_word(i + 3, "and") && i + 4 < m.code.len() {
        edit(m, m.code[i].s, m.code[i + 4].s, "")
    } else if i >= 2 && m.is_word(i - 1, "and") {
        edit(m, m.code[i - 2].e, m.code[i + 2].e, "")
    } else {
        return;
    };
    out.push(fix("Remove the check", vec![e]));
}

// ---------- tags and capabilities ----------

/// `t[0].w is a tag`: the column `w`, when one table has a `tag` column
/// of that name.
fn column_weapon(m: &Model, msg: &str, out: &mut Vec<Fix>) {
    let Some(x) = msg.split(", and ").nth(1).and_then(|r| r.split(" is a tag").next()) else {
        return;
    };
    let Some(col) = x.rsplit('.').next() else { return };
    let mut found = Vec::new();
    for item in &m.script.items {
        if let Item::Table { columns, .. } = item {
            found.extend(
                columns
                    .iter()
                    .filter(|(n, t)| n == col && t.name == "tag" && !t.optional)
                    .map(|(_, t)| t.pos),
            );
        }
    }
    if let [pos] = found[..] {
        let (s, e) = m.tok_at(pos);
        out.push(fix("Change the column to `weapon`", vec![edit(m, s, e, "weapon")]));
    }
}

/// `set_team needs capability player.team`: add it to the last
/// `requires` line, or a `requires` line after the header.
fn add_requires(m: &Model, msg: &str, out: &mut Vec<Fix>) {
    let Some(cap) = msg
        .split("capability ")
        .nth(1)
        .and_then(|r| r.split([':', ' ']).next())
        .map(|c| c.trim_end_matches('.'))
    else {
        return;
    };
    if caps::capability(cap, caps::API_VERSION).is_none() {
        return;
    }
    let script = m.script;
    let e = match script.requires.last() {
        Some((_, p)) => {
            let end = m.dotted_end(m.byte(*p));
            edit(m, end, end, format!(", {cap}"))
        }
        None => {
            let mut last = script.pos;
            for p in [script.carrier.as_ref().map(|c| c.1), script.teams.map(|t| t.1), script.list.as_ref().map(|l| l.1)]
                .into_iter()
                .flatten()
            {
                last = last.max(p);
            }
            let (off, before) = line_after(m, m.byte(last));
            edit(m, off, off, format!("{before}requires {cap}\n"))
        }
    };
    out.push(fix(format!("Add `{cap}` to `requires`"), vec![e]));
}

/// Removes the capability at `at` from its `requires` line, and the line
/// when it is the only one there.
fn remove_requires(m: &Model, at: usize, out: &mut Vec<Fix>) {
    let i = m.ci(at);
    let Some(t) = m.code.get(i) else { return };
    let (s, e) = (t.s, m.dotted_end(t.s));
    let j = m.ci(e);
    let cut = if m.is_punct(j, ",") {
        let next = m.code.get(j + 1).map_or(m.code[j].e, |t| t.s);
        edit(m, s, next, "")
    } else if i > 0 && m.is_punct(i - 1, ",") {
        edit(m, m.code[i - 1].s, e, "")
    } else {
        let line = m.li.line_of(s);
        edit(m, m.li.line_start(line), m.li.line_start(line + 1), "")
    };
    let name = &m.src[s..e];
    out.push(fix(format!("Remove `{name}` from `requires`"), vec![cut]));
}

// ---------- rename ----------

/// The edits that rename the name at `off`; see `ide::rename`.
pub(crate) fn rename(m: &Model, off: usize, new: &str) -> Result<Vec<TextEdit>, String> {
    let r = ref_near(m, off).ok_or_else(|| "there is no name to rename here".to_string())?;
    let def = &m.defs[r.def];
    let ident = new.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && new.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ident {
        return Err(format!("{new:?} is not a name: a letter or _, then letters, digits and _"));
    }
    if KEYWORDS.contains(&new) {
        return Err(format!("{new} is a keyword"));
    }
    if def.kind == DefKind::Palette {
        return Err("a palette is named by its string; edit the string".to_string());
    }
    if new == def.name {
        return Ok(Vec::new());
    }
    let script_level = matches!(
        def.kind,
        DefKind::Option
            | DefKind::Var(_)
            | DefKind::Table
            | DefKind::Enum
            | DefKind::Fn
            | DefKind::Label
            | DefKind::Const
    );
    if script_level && (m.top(new).is_some() || m.labels.contains_key(new)) {
        return Err(format!("the script declares {new} already"));
    }
    let mut edits: Vec<TextEdit> = m
        .refs
        .iter()
        .filter(|x| x.def == r.def)
        .map(|x| edit(m, x.s, x.e, new))
        .collect();
    // A label keeps its map text.
    if def.kind == DefKind::Label
        && let Some(l) = m.label_decl(r.def)
        && let Some(&(s, end)) = def.item.and_then(|i| m.items.get(i))
    {
        match &l.text {
            None => edits.push(edit(m, end, end, format!(" = \"{}\"", def.name))),
            // `= "finish"` says no more than the new name.
            Some(text) if text == new => {
                if let Some(eq) = (m.ci(s)..m.ci(end)).find(|&j| m.is_punct(j, "="))
                    && eq > 0
                {
                    edits.push(edit(m, m.code[eq - 1].e, end, ""));
                }
            }
            Some(_) => {}
        }
    }
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    edits.dedup();
    Ok(edits)
}

// ---------- the source ----------

/// The statement on the line holding `at` when it is one whole line of a
/// body: its span, without the indent, a trailing comment or spaces.
fn statement_line(m: &Model, at: usize) -> Option<(usize, usize)> {
    let line = m.li.line_of(at);
    let ls = m.li.line_start(line);
    let mut le = m.li.line_end(line);
    if let Some(c) = m.all.iter().find(|t| t.k == K::Comment && t.s >= ls && t.s < le) {
        le = c.s;
    }
    let text = &m.src[ls..le];
    let s = ls + (text.len() - text.trim_start().len());
    let e = ls + text.trim_end().len();
    if s >= e {
        return None;
    }
    let (a, b) = (m.ci(s), m.ci(e));
    let mut depth = 0i32;
    for j in a..b {
        if m.code[j].k != K::Punct {
            continue;
        }
        match m.text(&m.code[j]) {
            "{" | "}" | ";" => return None,
            "(" | "[" => depth += 1,
            ")" | "]" => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    if depth != 0 {
        return None;
    }
    let mut braces = 0i32;
    for t in &m.code[..a] {
        if t.k == K::Punct {
            match m.text(t) {
                "{" => braces += 1,
                "}" => braces -= 1,
                _ => {}
            }
        }
    }
    (braces > 0).then_some((s, e))
}

/// A name for a new binding: `seed`'s first letter, then with a number,
/// one no word near it and no script-level name already is.
fn fresh(m: &Model, near: &str, seed: &str) -> String {
    let first = seed
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .map_or_else(|| "x".to_string(), |c| c.to_ascii_lowercase().to_string());
    let taken = |n: &str| {
        near.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).any(|w| w == n)
            || m.top(n).is_some()
            || m.labels.contains_key(n)
            || KEYWORDS.contains(&n)
    };
    let mut name = first.clone();
    let mut k = 1;
    while taken(&name) {
        name = format!("{first}{k}");
        k += 1;
    }
    name
}

/// The innermost expression at `pos`.
fn expr_at(script: &Script, pos: Pos) -> Option<&Expr> {
    let mut found = None;
    walk_items(script, &mut |e| {
        if e.pos == pos {
            found = Some(e);
        }
    });
    found
}

/// The expression a field or index chain starts with.
fn leftmost(e: &Expr) -> &Expr {
    match &e.kind {
        ExprKind::Field(b, _) | ExprKind::Index(b, _) => leftmost(b),
        _ => e,
    }
}

/// Where a name, a call, a field or an index ends.
fn expr_end(m: &Model, e: &Expr) -> usize {
    let i = m.ci(m.byte(e.pos));
    let own = m.tok_at(e.pos).1;
    match &e.kind {
        ExprKind::Call(..) if m.is_punct(i + 1, "(") => m.partner(i + 1).map_or(own, |c| m.code[c].e),
        // A field's position is its dot.
        ExprKind::Field(..) => m.code.get(i + 1).map_or(own, |t| t.e),
        // An index's position is its `[`.
        ExprKind::Index(..) => m.partner(i).map_or(own, |c| m.code[c].e),
        _ => own,
    }
}

fn walk_items<'a>(script: &'a Script, f: &mut dyn FnMut(&'a Expr)) {
    for item in &script.items {
        match item {
            Item::Option(o) => {
                if let Some((lo, hi)) = &o.range {
                    walk_expr(lo, f);
                    walk_expr(hi, f);
                }
                walk_expr(&o.default, f);
            }
            Item::Var { init: Some(e), .. } => walk_expr(e, f),
            Item::Table { rows, .. } => {
                for cell in rows.iter().flatten() {
                    walk_expr(cell, f);
                }
            }
            Item::Fn(fd) => {
                if let Some(v) = &fd.value {
                    walk_expr(v, f);
                }
                walk_stmts(&fd.body, f);
            }
            Item::Const { value, .. } => walk_expr(value, f),
            Item::On { body, .. } => walk_stmts(body, f),
            _ => {}
        }
    }
}

fn walk_stmts<'a>(body: &'a [Stmt], f: &mut dyn FnMut(&'a Expr)) {
    for s in body {
        match s {
            Stmt::Let { value, .. } => walk_expr(value, f),
            Stmt::Assign { target, value, .. } => {
                walk_expr(target, f);
                walk_expr(value, f);
            }
            Stmt::Expr(e) => walk_expr(e, f),
            Stmt::If { cond, then, els } => {
                walk_expr(cond, f);
                walk_stmts(then, f);
                walk_stmts(els, f);
            }
            Stmt::IfLet {
                value, cond, then, els, ..
            } => {
                walk_expr(value, f);
                if let Some(c) = cond {
                    walk_expr(c, f);
                }
                walk_stmts(then, f);
                walk_stmts(els, f);
            }
            Stmt::ForEach { set, body, .. } => {
                walk_set(set, f);
                walk_stmts(body, f);
            }
            Stmt::Match { value, arms, els, .. } => {
                walk_expr(value, f);
                for arm in arms {
                    for p in &arm.patterns {
                        walk_expr(p, f);
                    }
                    walk_stmts(&arm.body, f);
                }
                if let Some(body) = els {
                    walk_stmts(body, f);
                }
            }
        }
    }
}

fn walk_set<'a>(set: &'a SetExpr, f: &mut dyn FnMut(&'a Expr)) {
    if let SetBaseExpr::Pick(inner, n) = &set.base {
        walk_set(inner, f);
        walk_expr(n, f);
    }
    if let Some(w) = &set.filter {
        walk_expr(w, f);
    }
}

fn walk_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Field(b, _) | ExprKind::Unary(_, b) | ExprKind::Team(b) | ExprKind::Text(b) | ExprKind::Is(b, _, _) => {
            walk_expr(b, f)
        }
        ExprKind::Index(a, b) | ExprKind::Binary(_, a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        ExprKind::Call(_, args) => {
            for a in args {
                walk_expr(a, f);
            }
        }
        ExprKind::Set(s) => walk_set(s, f),
        _ => {}
    }
}
