//! One read of a working copy against its context: the definitions, what
//! every atom names, and the problems. Every feature starts here.

use std::collections::HashMap;

use super::context::{BuiltinInfo, Context};
use super::syntax::{self, Lines, List, Node, Tree};
use super::{Diagnostic, severity};

/// A definition is a global or a script.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DefItem {
    Global,
    Script,
}

impl DefItem {
    pub fn name(self) -> &'static str {
        match self {
            DefItem::Global => "global",
            DefItem::Script => "script",
        }
    }
}

/// One top-level definition.
#[derive(Clone, Debug)]
pub(crate) struct Def {
    pub item: DefItem,
    pub name: String,
    pub name_span: (usize, usize),
    /// The whole `( ... )`.
    pub span: (usize, usize),
    /// The script kind, or `global`.
    pub kind: String,
    /// The return or value type; empty when the header has none.
    pub type_name: String,
    /// Where the value or the body starts in the list's items.
    pub body_from: usize,
}

/// What an atom is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// `global` or `script` at a definition's head.
    Keyword,
    /// A script kind in a header.
    Kind,
    /// A value type in a header.
    Type,
    /// A definition's own name (index into the definitions).
    DefName(usize),
    /// A call of a builtin (index into the context's builtins).
    CallBuiltin(usize),
    /// A call of a script.
    CallScript(usize),
    /// A call of a name that is neither.
    CallUnknown,
    /// A script named as a value (`(wake main)`).
    ScriptRef(usize),
    /// A global of this file.
    GlobalRef(usize),
    /// A game global (index into the context's external globals).
    ExtGlobal(usize),
    /// A scenario name of this value type.
    Name(String),
    Number,
    /// `true`, `false`, `none`.
    Constant,
    /// Anything else.
    Symbol,
}

/// An atom and what it is, with the parameter type it stands at when
/// the call says.
#[derive(Clone, Debug)]
pub(crate) struct Occ {
    pub start: usize,
    pub end: usize,
    pub role: Role,
    pub param: Option<String>,
}

pub(crate) struct Model<'a> {
    pub src: &'a str,
    pub ctx: &'a Context,
    pub lines: Lines<'a>,
    pub tree: Tree,
    pub defs: Vec<Def>,
    pub occs: Vec<Occ>,
    pub diagnostics: Vec<Diagnostic>,
    builtins: HashMap<String, usize>,
    externals: HashMap<&'a str, usize>,
    /// Each scenario name and the first value type listing it.
    names: HashMap<&'a str, &'a str>,
}

pub(crate) fn is_number(s: &str) -> bool {
    let t = s.strip_prefix('-').unwrap_or(s);
    !t.is_empty()
        && t.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.')
        && t.parse::<f64>().is_ok()
}

/// The text of a symbol as the scripts spell it: an atom, or a quoted
/// string without its quotes.
pub(crate) fn spelled<'a>(src: &'a str, n: &Node) -> Option<&'a str> {
    match n {
        Node::Atom { start, end } => Some(&src[*start..*end]),
        Node::Str { start, end } if end - start >= 2 && src[*start..*end].ends_with('"') => {
            Some(&src[start + 1..end - 1])
        }
        _ => None,
    }
}

impl<'a> Model<'a> {
    pub fn new(src: &'a str, ctx: &'a Context) -> Self {
        let tree = syntax::parse(src);
        let mut builtins = HashMap::new();
        for (i, b) in ctx.builtins.iter().enumerate() {
            builtins.insert(format!("#builtin:{}", b.index), i);
            if let Some(n) = &b.name {
                builtins.insert(n.clone(), i);
            }
        }
        let externals = ctx
            .external_globals
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.as_str(), i))
            .collect();
        let mut names = HashMap::new();
        for (ty, list) in &ctx.names {
            for n in list {
                names.entry(n.as_str()).or_insert(ty.as_str());
            }
        }
        let mut m = Model {
            src,
            ctx,
            lines: Lines::new(src),
            tree,
            defs: Vec::new(),
            occs: Vec::new(),
            diagnostics: Vec::new(),
            builtins,
            externals,
            names,
        };
        for e in m.tree.errors.clone() {
            m.problem(e.start, e.end, severity::ERROR, e.code, e.message);
        }
        if ctx.is_empty() {
            m.problem(
                0,
                0,
                severity::INFORMATION,
                "context.missing",
                format!(
                    "No scenario context ({}) beside this file: only the syntax is checked. Open the script from Studio's Scripts tab for its builtins and names.",
                    super::CONTEXT_FILE
                ),
            );
        }
        m.read_definitions();
        m.read_bodies();
        m.diagnostics.sort_by_key(|d| (d.range.start, d.severity));
        m
    }

    pub fn builtin(&self, name: &str) -> Option<(usize, &'a BuiltinInfo)> {
        self.builtins.get(name).map(|&i| (i, &self.ctx.builtins[i]))
    }

    pub fn script(&self, name: &str) -> Option<usize> {
        self.defs.iter().position(|d| d.item == DefItem::Script && d.name == name)
    }

    pub fn global(&self, name: &str) -> Option<usize> {
        self.defs.iter().position(|d| d.item == DefItem::Global && d.name == name)
    }

    pub fn external(&self, name: &str) -> Option<usize> {
        self.externals.get(name).copied()
    }

    /// The atom at or just before `off`.
    pub fn occ_at(&self, off: usize) -> Option<&Occ> {
        self.occs
            .iter()
            .find(|o| o.start <= off && off <= o.end && o.start < o.end)
    }

    /// The definition holding `off`.
    pub fn def_at(&self, off: usize) -> Option<usize> {
        self.defs.iter().position(|d| d.span.0 <= off && off < d.span.1.max(d.span.0 + 1))
    }

    fn problem(&mut self, start: usize, end: usize, sev: u8, code: &str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            range: self.lines.range(start, end),
            severity: sev,
            code: code.to_owned(),
            source: "h2 hs",
            message: message.into(),
        });
    }

    fn occ(&mut self, n: &Node, role: Role) {
        let (start, end) = n.span();
        self.occs.push(Occ { start, end, role, param: None });
    }

    fn known_type(&self, t: &str) -> bool {
        self.ctx.types.is_empty() || self.ctx.types.iter().any(|x| x == t)
    }

    /// Whether a kind's header names a return type. With no context, a
    /// header with two atoms before the body is read as typed.
    fn typed(&self, kind: &str, l: &List) -> bool {
        if !self.ctx.script_kinds.is_empty() {
            return self.ctx.typed_kinds.iter().any(|k| k == kind);
        }
        l.items.len() >= 5 && l.items[2].atom(self.src).is_some() && l.items[3].atom(self.src).is_some()
    }

    fn read_definitions(&mut self) {
        let items = std::mem::take(&mut self.tree.items);
        let src = self.src;
        for n in &items {
            let Node::List(l) = n else {
                let (s, e) = n.span();
                self.problem(s, e, severity::ERROR, "def.expected", "expected (global ...) or (script ...)");
                continue;
            };
            let Some(head) = l.items.first().and_then(|h| h.atom(src)) else {
                self.problem(l.open, l.open + 1, severity::ERROR, "def.expected", "expected (global ...) or (script ...)");
                continue;
            };
            let at_head = l.items[0].span();
            match head {
                "global" => {
                    self.occ(&l.items[0], Role::Keyword);
                    let (ty, name) = (l.items.get(1), l.items.get(2));
                    let (Some(ty_atom), Some(name_atom)) =
                        (ty.and_then(|t| t.atom(src)), name.and_then(|n| n.atom(src)))
                    else {
                        self.problem(at_head.0, at_head.1, severity::ERROR, "def.malformed", "a global is (global <type> <name> <value>)");
                        continue;
                    };
                    self.occ(ty.unwrap(), Role::Type);
                    if !self.known_type(ty_atom) {
                        let (s, e) = ty.unwrap().span();
                        self.problem(s, e, severity::ERROR, "type.unknown", format!("no value type is named {ty_atom}"));
                    }
                    if l.items.len() != 4 {
                        self.problem(at_head.0, at_head.1, severity::ERROR, "def.malformed", "a global is (global <type> <name> <value>): one value");
                    }
                    self.define(DefItem::Global, name_atom, name.unwrap(), l, "global", ty_atom, 3);
                }
                "script" => {
                    self.occ(&l.items[0], Role::Keyword);
                    let Some(kind_node) = l.items.get(1) else {
                        self.problem(at_head.0, at_head.1, severity::ERROR, "def.malformed", "a script is (script <kind> <name> <expression>...)");
                        continue;
                    };
                    let Some(kind) = kind_node.atom(src) else {
                        let (s, e) = kind_node.span();
                        self.problem(s, e, severity::ERROR, "def.malformed", "a script's kind comes after `script`");
                        continue;
                    };
                    self.occ(kind_node, Role::Kind);
                    if !self.ctx.script_kinds.is_empty() && !self.ctx.script_kinds.iter().any(|k| k == kind) {
                        let (s, e) = kind_node.span();
                        self.problem(s, e, severity::ERROR, "script.kind", format!("no script kind is named {kind} (there are: {})", self.ctx.script_kinds.join(", ")));
                    }
                    let typed = self.typed(kind, l);
                    let mut at = 2;
                    let mut type_name = String::new();
                    if typed {
                        match l.items.get(2).and_then(|t| t.atom(src)) {
                            Some(t) => {
                                self.occ(&l.items[2], Role::Type);
                                if !self.known_type(t) {
                                    let (s, e) = l.items[2].span();
                                    self.problem(s, e, severity::ERROR, "type.unknown", format!("no value type is named {t}"));
                                }
                                type_name = t.to_owned();
                            }
                            None => {
                                self.problem(at_head.0, at_head.1, severity::ERROR, "def.malformed", format!("a {kind} script is (script {kind} <type> <name> <expression>...)"));
                                continue;
                            }
                        }
                        at = 3;
                    }
                    let Some(name_atom) = l.items.get(at).and_then(|n| n.atom(src)) else {
                        self.problem(at_head.0, at_head.1, severity::ERROR, "def.malformed", "a script names itself after its kind");
                        continue;
                    };
                    if l.items.len() <= at + 1 {
                        self.problem(at_head.0, at_head.1, severity::ERROR, "def.malformed", format!("script {name_atom} has no body"));
                    }
                    if type_name.is_empty() {
                        type_name = "void".into();
                    }
                    self.define(DefItem::Script, name_atom, &l.items[at], l, kind, &type_name, at + 1);
                }
                other => {
                    self.problem(at_head.0, at_head.1, severity::ERROR, "def.unknown", format!("{other} is not a definition: expected (global ...) or (script ...)"));
                }
            }
        }
        self.tree.items = items;
    }

    #[allow(clippy::too_many_arguments)]
    fn define(&mut self, item: DefItem, name: &str, name_node: &Node, l: &List, kind: &str, ty: &str, body_from: usize) {
        if self.defs.iter().any(|d| d.item == item && d.name == name) {
            let (s, e) = name_node.span();
            self.problem(s, e, severity::ERROR, "def.duplicate", format!("another {} is named {name}", item.name()));
        }
        let index = self.defs.len();
        self.defs.push(Def {
            item,
            name: name.to_owned(),
            name_span: name_node.span(),
            span: (l.open, l.end),
            kind: kind.to_owned(),
            type_name: ty.to_owned(),
            body_from,
        });
        self.occ(name_node, Role::DefName(index));
    }

    fn read_bodies(&mut self) {
        let items = std::mem::take(&mut self.tree.items);
        let defs = self.defs.clone();
        for d in &defs {
            let Some(l) = items
                .iter()
                .filter_map(Node::list)
                .find(|l| l.open == d.span.0)
            else {
                continue;
            };
            for n in l.items.iter().skip(d.body_from) {
                self.expression(n, None, false);
            }
        }
        self.tree.items = items;
    }

    /// One expression; `param` is the type its call asks for there.
    /// `clause`: a `cond` clause, whose first item is a test, not a name.
    fn expression(&mut self, n: &Node, param: Option<&str>, clause: bool) {
        match n {
            Node::Str { .. } => {
                if let Some(p) = param {
                    self.check_name(n, p);
                }
            }
            Node::Atom { .. } => self.value(n, param),
            Node::List(l) => {
                if clause {
                    for it in &l.items {
                        self.expression(it, None, false);
                    }
                    return;
                }
                self.call(l);
            }
        }
    }

    fn value(&mut self, n: &Node, param: Option<&str>) {
        let text = n.atom(self.src).unwrap_or("");
        let role = if is_number(text) {
            Role::Number
        } else if matches!(text, "true" | "false" | "none") {
            Role::Constant
        } else if let Some(g) = self.global(text) {
            Role::GlobalRef(g)
        } else if let Some(g) = self.external(text) {
            Role::ExtGlobal(g)
        } else if let Some(name_ty) = self.scenario_type(text, param) {
            Role::Name(name_ty)
        } else if let Some(s) = self.script(text) {
            Role::ScriptRef(s)
        } else {
            Role::Symbol
        };
        if matches!(role, Role::Symbol)
            && let Some(p) = param
        {
            self.check_name(n, p);
        }
        let (start, end) = n.span();
        self.occs.push(Occ {
            start,
            end,
            role,
            param: param.map(str::to_owned),
        });
    }

    /// The type of scenario name `text` is, preferring the parameter's.
    fn scenario_type(&self, text: &str, param: Option<&str>) -> Option<String> {
        if let Some(p) = param
            && self.ctx.names_for(p).is_some_and(|ns| ns.iter().any(|n| n == text))
        {
            return Some(p.to_owned());
        }
        self.names.get(text).map(|t| (*t).to_owned())
    }

    /// A value at an object or trigger volume parameter must be one of
    /// the scenario's names, when the context lists them.
    fn check_name(&mut self, n: &Node, param: &str) {
        let is_object = self.ctx.is_object_type(param);
        if !is_object && param != "trigger_volume" {
            return;
        }
        let Some(names) = self.ctx.names_for(param) else {
            return;
        };
        let Some(text) = spelled(self.src, n) else {
            return;
        };
        if text == "none" || names.iter().any(|x| x == text) || self.global(text).is_some() || self.external(text).is_some() {
            return;
        }
        let (s, e) = n.span();
        let what = if is_object { "object" } else { "trigger volume" };
        self.problem(s, e, severity::WARNING, "name.unknown", format!("no {what} in {} is named {text}", self.map_name()));
    }

    fn map_name(&self) -> String {
        if self.ctx.map.is_empty() { "the map".into() } else { self.ctx.map.clone() }
    }

    fn call(&mut self, l: &List) {
        let Some(head) = l.items.first() else {
            self.problem(l.open, l.end, severity::ERROR, "call.empty", "an empty call: () names no function");
            return;
        };
        let Some(name) = head.atom(self.src) else {
            let (s, e) = head.span();
            self.problem(s, e, severity::ERROR, "call.head", "a call starts with the name of a builtin or script");
            for it in &l.items {
                self.expression(it, None, false);
            }
            return;
        };
        let args = &l.items[1..];
        if let Some((bi, b)) = self.builtin(name) {
            self.occ(head, Role::CallBuiltin(bi));
            let clause = name == "cond";
            if b.own_arguments {
                for a in args {
                    self.expression(a, None, clause);
                }
                return;
            }
            if args.len() != b.parameters.len() {
                let (s, e) = head.span();
                self.problem(
                    s,
                    e,
                    severity::ERROR,
                    "call.arity",
                    format!(
                        "{name} takes {} argument{}{}, not {}",
                        b.parameters.len(),
                        if b.parameters.len() == 1 { "" } else { "s" },
                        if b.parameters.is_empty() { String::new() } else { format!(" ({})", b.call_form()) },
                        args.len()
                    ),
                );
            }
            let params = b.parameters.clone();
            for (i, a) in args.iter().enumerate() {
                self.expression(a, params.get(i).map(String::as_str), false);
            }
        } else if let Some(s) = self.script(name) {
            self.occ(head, Role::CallScript(s));
            if !args.is_empty() {
                let (hs, he) = head.span();
                self.problem(hs, he, severity::ERROR, "call.arity", format!("script {name} takes no arguments"));
            }
            for a in args {
                self.expression(a, None, false);
            }
        } else {
            self.occ(head, Role::CallUnknown);
            if !self.ctx.is_empty() {
                let (s, e) = head.span();
                let mut message = format!("no builtin or script is named {name}");
                if let Some(near) = self.nearest(name) {
                    message.push_str(&format!(": did you mean {near}?"));
                }
                self.problem(s, e, severity::ERROR, "call.unknown", message);
            }
            for a in args {
                self.expression(a, None, false);
            }
        }
    }

    /// The builtin or script name closest to `name`, within two edits.
    pub fn nearest(&self, name: &str) -> Option<String> {
        let names = self
            .ctx
            .builtins
            .iter()
            .filter_map(|b| b.name.as_deref())
            .chain(self.defs.iter().filter(|d| d.item == DefItem::Script).map(|d| d.name.as_str()));
        let mut best: Option<(usize, &str)> = None;
        for n in names {
            if n.len().abs_diff(name.len()) > 2 {
                continue;
            }
            let d = distance(n, name);
            if d <= 2 && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, n));
            }
        }
        best.map(|(_, n)| n.to_owned())
    }
}

/// Levenshtein distance over chars.
pub(crate) fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}
