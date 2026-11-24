//! The script compiler: Lisp-style source, as the decompiler prints it or
//! a person writes it, to syntax nodes in a scenario's tables
//! (docs/formats/haloscript.md, "Compiling source";
//! docs/editor/rich-editors.md, "Scripts").
//!
//! [`compile`] reads every `(global ...)` and `(script ...)` in the source.
//! A definition with the name of one in the scenario replaces it, keeping
//! its index; any other is added at the end of its table. The replaced
//! definitions' nodes are freed, and the new nodes take free slots first,
//! then new slots. Every problem is a [`Message`] with its place, and
//! nothing is returned unless the whole source compiles and the new tables
//! pass [`Scenario::validate`].
//!
//! Node types follow the parent: an argument of a builtin with a fixed
//! signature takes the parameter's type, a script's body its return type, a
//! global's value the global's type. A builtin that reads its own
//! arguments (flags bit 1: `begin`, `if` and the like) has no parameter
//! list; its arguments take the types this map's own calls of it give them
//! (`Rule`), and otherwise their own type. A value of another type is
//! accepted only where [`castable`] has the cast.

use std::collections::{BTreeSet, HashMap, hash_map::Entry};

use crate::cast::{castable, types};
use crate::expression::global_word;
use crate::ide::syntax::{self, Lines, List, Node as Syn};
use crate::{
    Catalog, Datum, Global, Item, MAX_NODES, MAX_STRING_BYTES, MAX_TOKEN_BYTES, Naming, Node,
    RETAIL_NODES, Result, Scenario, Script, invalid, script_kind_has_type,
};

/// The type of a call's function-name atom.
const FUNCTION_NAME: u16 = 2;
/// The `script` value type: a literal naming a script (`(wake main)`).
/// The first type that is not a plain value; every type from here on is
/// spelled by a name the compiler looks up.
const SCRIPT: u16 = 10;
/// Node flags, with the retained bit (3) every compiled node carries.
const CALL: u16 = 8;
const ATOM: u16 = 9;
const SCRIPT_CALL: u16 = 10;
const GLOBAL: u16 = 13;
/// Deepest nesting of calls the compiler follows.
pub const MAX_DEPTH: usize = 200;

/// A problem in the source, with its place: a byte span, and a 1-based
/// line and column (columns count UTF-16 units, as editors do).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
    /// Stable: `syntax.open`, `def.kind`, `call.arity`, `type.cast`, ...
    pub code: &'static str,
    pub text: String,
}

impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.text)
    }
}

/// Resolves a name of a scenario thing (an object, a trigger volume, AI,
/// ...) the map's scripts do not already use: `(type, type name, token)`
/// to the literal's value bits.
pub type Resolve<'a> = &'a dyn Fn(u16, &str, &[u8]) -> Option<u32>;

/// What the compiler may use beyond the scenario and the catalog.
#[derive(Default, Clone, Copy)]
pub struct Options<'a> {
    /// Builtin names; without it, the catalog's. The names the scenario's
    /// own calls use always count.
    pub naming: Option<&'a Naming>,
    /// Most node slots the tables may hold; [`RETAIL_NODES`] by default.
    /// A scenario already larger keeps its size as the limit.
    pub node_limit: Option<usize>,
    /// Names of scenario things, read from its blocks by the caller.
    pub resolve: Option<Resolve<'a>>,
}

/// The scenario with the source's definitions in it, and what changed.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub scenario: Scenario,
    /// Each definition of the source, in source order.
    pub items: Vec<Item>,
    /// Nodes the compiled definitions use.
    pub nodes: usize,
    /// Slots freed from the replaced definitions.
    pub freed: usize,
    /// Free slots the new nodes took.
    pub reused: usize,
    /// Slots added to the node table.
    pub grown: usize,
    /// Bytes added to the string data.
    pub new_strings: usize,
}

/// Compile `source` into a copy of `scenario`. See the module comment.
pub fn compile<'a>(
    scenario: &'a Scenario,
    catalog: &'a Catalog,
    source: &'a str,
    options: &Options<'a>,
) -> std::result::Result<Compiled, Vec<Message>> {
    let lines = Lines::new(source);
    let tree = syntax::parse(source);
    if !tree.errors.is_empty() {
        return Err(tree
            .errors
            .iter()
            .map(|e| message(&lines, e.start, e.end, e.code, e.message.clone()))
            .collect());
    }
    let mut cx = Cx::new(scenario, catalog, source, options);
    let defs = cx.headers(&tree.items);
    if cx.errors.is_empty() {
        cx.names(&defs);
        let roots: Vec<Option<usize>> = defs.iter().map(|d| cx.body(d)).collect();
        if cx.errors.is_empty() {
            let roots = roots.into_iter().flatten().collect::<Vec<_>>();
            match cx.finish(&defs, &roots) {
                Ok(compiled) => return Ok(compiled),
                Err(raw) => cx.errors.push(raw),
            }
        }
    }
    let mut errors = cx.errors;
    errors.sort_by_key(|e| e.start);
    Err(errors
        .into_iter()
        .map(|e| message(&lines, e.start, e.end, e.code, e.text))
        .collect())
}

fn message(lines: &Lines<'_>, start: usize, end: usize, code: &'static str, text: String) -> Message {
    let at = lines.position(start);
    Message {
        start,
        end,
        line: at.line + 1,
        column: at.character + 1,
        code,
        text,
    }
}

/// The first place two expression trees differ in what the compiler
/// promises to reproduce: shape, and each node's opcode, type, flags and
/// value. Call data is compared by shape; function names, globals and
/// symbols also by spelling; string literals by their bytes only (their
/// data word is rewritten when the map loads). `None` when they agree.
pub fn first_difference(a: &Scenario, ra: Datum, b: &Scenario, rb: Datum) -> Result<Option<String>> {
    let mut stack = vec![(ra, rb)];
    let mut steps = 0usize;
    while let Some((x, y)) = stack.pop() {
        match (x == Datum::NULL, y == Datum::NULL) {
            (true, true) => continue,
            (false, false) => {}
            _ => {
                return Ok(Some(format!(
                    "slots {} and {}: one argument list is longer",
                    x.slot(),
                    y.slot()
                )));
            }
        }
        steps += 1;
        if steps > a.nodes.len() + b.nodes.len() {
            return Err(invalid("cyclic expression"));
        }
        let (m, n) = (*a.node(x)?, *b.node(y)?);
        let at = format!("slots {} and {}", x.slot(), y.slot());
        if (m.opcode, m.value_type, m.flags) != (n.opcode, n.value_type, n.flags) {
            return Ok(Some(format!(
                "{at}: opcode, type and flags {:?} and {:?}",
                (m.opcode, m.value_type, m.flags),
                (n.opcode, n.value_type, n.flags)
            )));
        }
        if m.atom() {
            let head = m.value_type == FUNCTION_NAME;
            let string = !m.global() && !head && m.opcode == types::STRING;
            let spelled = head || string || m.global() || m.opcode >= SCRIPT;
            if spelled && a.string_bytes(m.string_offset)? != b.string_bytes(n.string_offset)? {
                return Ok(Some(format!("{at}: spellings differ")));
            }
            if !head && !string && m.data != n.data {
                return Ok(Some(format!(
                    "{at}: data {:#010x} and {:#010x}",
                    m.data, n.data
                )));
            }
        } else {
            stack.push((Datum(m.data), Datum(n.data)));
        }
        stack.push((m.next, n.next));
    }
    Ok(None)
}

/// A problem before it has a line and column.
struct Raw {
    start: usize,
    end: usize,
    code: &'static str,
    text: String,
}

/// One definition's header.
struct Def<'t> {
    global: bool,
    name: String,
    span: (usize, usize),
    kind: u16,
    /// The global's type or the script's return type.
    ty: u16,
    body: &'t [Syn],
    /// Index in its table.
    index: usize,
    /// Whether it replaces the scenario's definition at `index`.
    replaces: bool,
}

/// A node before it has a slot. `child` and `next` index the arena.
struct New {
    opcode: u16,
    ty: u16,
    flags: u16,
    string: u32,
    data: u32,
    child: Option<usize>,
    next: Option<usize>,
}

/// A literal's source type and value bits, and whether it keeps its token.
struct Lit {
    source: u16,
    data: u32,
    token: bool,
}

/// Where an argument stands among a call's arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Pos {
    Only,
    First,
    Middle,
    Last,
}

fn position(k: usize, len: usize) -> Pos {
    match (k, len) {
        (_, 1) => Pos::Only,
        (0, _) => Pos::First,
        (k, len) if k + 1 == len => Pos::Last,
        _ => Pos::Middle,
    }
}

/// The type an argument of a builtin that reads its own arguments takes,
/// learned from the map's calls of it at that position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// Its call's type, wherever every call seen had the argument at it.
    Parent,
    /// One type, wherever every call seen had the argument at it.
    Fixed(u16),
    /// Its own type: the calls seen agree on nothing else.
    Natural,
}

#[derive(Clone, Copy, Debug)]
struct Seen {
    fixed: u16,
    fixed_ok: bool,
    parent_ok: bool,
}

impl Seen {
    fn rule(self) -> Rule {
        if self.parent_ok {
            Rule::Parent
        } else if self.fixed_ok {
            Rule::Fixed(self.fixed)
        } else {
            Rule::Natural
        }
    }
}

/// Record `value` under `key`; a second, different value makes the key
/// ambiguous (`None`).
fn merge<K: Eq + std::hash::Hash, V: PartialEq>(map: &mut HashMap<K, Option<V>>, key: K, value: V) {
    match map.entry(key) {
        Entry::Vacant(e) => {
            e.insert(Some(value));
        }
        Entry::Occupied(mut e) => {
            if e.get().as_ref() != Some(&value) {
                e.insert(None);
            }
        }
    }
}

/// `None` where any type is taken as it is: no expectation, or passthrough.
fn want(expected: Option<u16>) -> Option<u16> {
    expected.filter(|&t| t != types::PASSTHROUGH)
}

struct Cx<'a> {
    src: &'a str,
    scenario: &'a Scenario,
    catalog: &'a Catalog,
    resolve: Option<Resolve<'a>>,
    node_limit: Option<usize>,
    /// Name to builtin index; `None` where two builtins share it.
    builtins: HashMap<String, Option<u16>>,
    /// Each builtin's name, if it has one.
    named: Vec<Option<String>>,
    begin: Option<u16>,
    /// Name to (index, return type) and (index, type).
    scripts: HashMap<String, Option<(u16, u16)>>,
    globals: HashMap<String, Option<(u16, u16)>>,
    /// Game global name to (data word, type), as the map's scripts use them.
    externals: HashMap<String, Option<(u32, u16)>>,
    /// Symbol literals of the map's scripts: (node type, token) to
    /// (source type, data); (source type, token) to data; token alone.
    by_value: HashMap<(u16, Vec<u8>), Option<(u16, u32)>>,
    by_source: HashMap<(u16, Vec<u8>), Option<u32>>,
    by_spelling: HashMap<Vec<u8>, Option<(u16, u32)>>,
    /// The upper half of script literals' data, where the map agrees on one.
    script_high: u32,
    rules: HashMap<(u16, Pos), Seen>,
    strings: Vec<u8>,
    interned: HashMap<Vec<u8>, u32>,
    arena: Vec<New>,
    errors: Vec<Raw>,
}

impl<'a> Cx<'a> {
    fn new(scenario: &'a Scenario, catalog: &'a Catalog, src: &'a str, options: &Options<'a>) -> Self {
        let count = catalog.functions().len();
        let mut named: Vec<Option<String>> = (0..count)
            .map(|i| match options.naming {
                Some(n) => n.name(i).map(str::to_owned),
                None => catalog.functions()[i].name().map(str::to_owned),
            })
            .collect();
        // The names the map's own calls spell win: a new call is spelled
        // as the old ones are, or the tables would disagree on a name.
        if let Ok(own) = scenario.builtin_names() {
            for (i, name) in own {
                if let Some(slot) = named.get_mut(usize::from(i)) {
                    *slot = Some(name);
                }
            }
        }
        let mut builtins = HashMap::new();
        for (i, name) in named.iter().enumerate() {
            if let Some(name) = name {
                merge(&mut builtins, name.clone(), i as u16);
            }
        }
        let begin = builtins.get("begin").copied().flatten();
        let mut cx = Cx {
            src,
            scenario,
            catalog,
            resolve: options.resolve,
            node_limit: options.node_limit,
            builtins,
            named,
            begin,
            scripts: HashMap::new(),
            globals: HashMap::new(),
            externals: HashMap::new(),
            by_value: HashMap::new(),
            by_source: HashMap::new(),
            by_spelling: HashMap::new(),
            script_high: 0,
            rules: HashMap::new(),
            strings: scenario.strings.clone(),
            interned: HashMap::new(),
            arena: Vec::new(),
            errors: Vec::new(),
        };
        cx.harvest();
        let mut at = 0usize;
        for piece in scenario.strings.split(|&b| b == 0) {
            if at + piece.len() < scenario.strings.len() {
                cx.interned.entry(piece.to_vec()).or_insert(at as u32);
            }
            at += piece.len() + 1;
        }
        cx
    }

    /// What the map's scripts already say: game global names, symbol
    /// values, and the argument types of builtins that read their own.
    fn harvest(&mut self) {
        let s = self.scenario;
        let mut highs = BTreeSet::new();
        for n in s.nodes.iter().filter(|n| n.live()) {
            if n.atom() {
                if n.value_type == FUNCTION_NAME {
                    continue;
                }
                let Ok(token) = s.string_bytes(n.string_offset) else {
                    continue;
                };
                if n.global() {
                    let Ok(raw) = global_word(n.data) else { continue };
                    let Some(&ty) = (raw & 0x8000 != 0)
                        .then(|| self.catalog.external_global_types().get(usize::from(raw & 0x7fff)))
                        .flatten()
                    else {
                        continue;
                    };
                    if let Ok(name) = std::str::from_utf8(token) {
                        merge(&mut self.externals, name.to_owned(), (n.data, ty));
                    }
                } else if n.opcode >= SCRIPT {
                    merge(&mut self.by_value, (n.value_type, token.to_vec()), (n.opcode, n.data));
                    merge(&mut self.by_source, (n.opcode, token.to_vec()), n.data);
                    merge(&mut self.by_spelling, token.to_vec(), (n.opcode, n.data));
                    if n.opcode == SCRIPT {
                        highs.insert(n.data & 0xffff_0000);
                    }
                }
                continue;
            }
            if n.script_call() {
                continue;
            }
            let Some(f) = self.catalog.functions().get(usize::from(n.opcode)) else {
                continue;
            };
            if f.flags() & 2 == 0 {
                continue;
            }
            let Ok(head) = s.node(Datum(n.data)) else { continue };
            let mut args = Vec::new();
            let mut next = head.next;
            while next != Datum::NULL && args.len() < s.nodes.len() {
                let Ok(a) = s.node(next) else { break };
                args.push(*a);
                next = a.next;
            }
            for (k, a) in args.iter().enumerate() {
                let seen = self
                    .rules
                    .entry((n.opcode, position(k, args.len())))
                    .or_insert(Seen {
                        fixed: a.value_type,
                        fixed_ok: true,
                        parent_ok: true,
                    });
                seen.fixed_ok &= seen.fixed == a.value_type;
                seen.parent_ok &= a.value_type == n.value_type;
            }
        }
        if highs.len() == 1 {
            self.script_high = highs.into_iter().next().unwrap_or(0);
        }
    }

    fn err(&mut self, (start, end): (usize, usize), code: &'static str, text: impl Into<String>) {
        self.errors.push(Raw {
            start,
            end,
            code,
            text: text.into(),
        });
    }

    fn type_name(&self, t: u16) -> String {
        self.catalog
            .type_name(t)
            .map_or_else(|_| format!("type {t}"), str::to_owned)
    }

    fn label(&self, index: u16) -> String {
        self.named
            .get(usize::from(index))
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("#builtin:{index}"))
    }

    /// An atom's or a quoted string's bytes.
    fn spelling(&mut self, n: &Syn) -> Option<Vec<u8>> {
        let span = n.span();
        let bytes = match n {
            Syn::Atom { start, end } => self.src.as_bytes()[*start..*end].to_vec(),
            Syn::Str { start, end } => match unescape(&self.src[*start..*end]) {
                Ok(b) => b,
                Err(text) => {
                    self.err(span, "syntax.escape", text);
                    return None;
                }
            },
            Syn::List(_) => {
                self.err(span, "syntax.list", "a name or a value is wanted here, not a list");
                return None;
            }
        };
        if bytes.len() > MAX_TOKEN_BYTES {
            self.err(span, "value.long", format!("a token holds at most {MAX_TOKEN_BYTES} bytes"));
            return None;
        }
        Some(bytes)
    }

    /// An atom or quoted name as text.
    fn word(&mut self, list: &List, at: usize, code: &'static str, what: &str) -> Option<(String, (usize, usize))> {
        let Some(n) = list.items.get(at) else {
            self.err((list.open, list.end), code, format!("this definition has no {what}"));
            return None;
        };
        let span = n.span();
        let bytes = self.spelling(n)?;
        match String::from_utf8(bytes) {
            Ok(s) if !s.is_empty() => Some((s, span)),
            _ => {
                self.err(span, code, format!("this is not a {what}"));
                None
            }
        }
    }

    fn headers<'t>(&mut self, items: &'t [Syn]) -> Vec<Def<'t>> {
        let mut defs: Vec<Def<'t>> = Vec::new();
        let mut next = [self.scenario.globals.len(), self.scenario.scripts.len()];
        for item in items {
            let span = item.span();
            let Some(list) = item.list() else {
                self.err(span, "def.head", "expected `(global ...)` or `(script ...)` here");
                continue;
            };
            let global = match list.items.first().and_then(|n| n.atom(self.src)) {
                Some("global") => true,
                Some("script") => false,
                _ => {
                    self.err(span, "def.head", "a definition starts with `global` or `script`");
                    continue;
                }
            };
            let mut at = 1;
            let mut kind = 0u16;
            let mut typed = true;
            if !global {
                let Some((word, wspan)) = self.word(list, at, "def.kind", "script kind") else {
                    continue;
                };
                let Some(k) = self.catalog.script_kinds().iter().position(|k| *k == word) else {
                    let kinds = self.catalog.script_kinds().join(", ");
                    self.err(wspan, "def.kind", format!("`{word}` is not a script kind: {kinds}"));
                    continue;
                };
                kind = k as u16;
                typed = script_kind_has_type(kind);
                at += 1;
            }
            let mut ty = None;
            if typed {
                let Some((word, wspan)) = self.word(list, at, "def.type", "type") else {
                    continue;
                };
                let Some(t) = self.catalog.types().iter().position(|t| *t == word) else {
                    self.err(wspan, "def.type", format!("`{word}` is not a type of this executable"));
                    continue;
                };
                ty = Some(t as u16);
                at += 1;
            }
            let Some((name, nspan)) = self.word(list, at, "def.name", "name") else {
                continue;
            };
            at += 1;
            if name.len() >= 32 || name.contains('\0') {
                self.err(nspan, "def.name", format!("`{name}` is too long: a name is at most 31 bytes"));
                continue;
            }
            let body = &list.items[at.min(list.items.len())..];
            if body.is_empty() {
                self.err(span, "def.body", format!("`{name}` has no body"));
                continue;
            }
            if global && body.len() > 1 {
                self.err(body[1].span(), "def.body", format!("the global `{name}` has one value"));
                continue;
            }
            if defs.iter().any(|d| d.global == global && d.name == name) {
                self.err(nspan, "def.duplicate", format!("`{name}` is defined twice in this source"));
                continue;
            }
            let existing: Vec<usize> = if global {
                (self.scenario.globals.iter().enumerate())
                    .filter(|(_, g)| g.name().ok() == Some(name.as_str()))
                    .map(|(i, _)| i)
                    .collect()
            } else {
                (self.scenario.scripts.iter().enumerate())
                    .filter(|(_, s)| s.name().ok() == Some(name.as_str()))
                    .map(|(i, _)| i)
                    .collect()
            };
            if existing.len() > 1 {
                self.err(nspan, "def.ambiguous", format!("the map has more than one `{name}`"));
                continue;
            }
            let replaces = existing.len() == 1;
            let index = existing.first().copied().unwrap_or_else(|| {
                let table = usize::from(!global);
                next[table] += 1;
                next[table] - 1
            });
            let ty = ty.unwrap_or_else(|| {
                if replaces {
                    self.scenario.scripts[index].return_type
                } else {
                    types::VOID
                }
            });
            defs.push(Def {
                global,
                name,
                span,
                kind,
                ty,
                body,
                index,
                replaces,
            });
        }
        if next[0] > crate::MAX_DEFINITIONS || next[1] > crate::MAX_DEFINITIONS {
            self.err((0, 0), "def.count", format!("a table holds at most {} definitions", crate::MAX_DEFINITIONS));
        }
        defs
    }

    /// Script and global names: the scenario's, then the source's.
    fn names(&mut self, defs: &[Def<'_>]) {
        for (i, s) in self.scenario.scripts.iter().enumerate() {
            if let Ok(name) = s.name() {
                merge(&mut self.scripts, name.to_owned(), (i as u16, s.return_type));
            }
        }
        for (i, g) in self.scenario.globals.iter().enumerate() {
            if let Ok(name) = g.name() {
                merge(&mut self.globals, name.to_owned(), (i as u16, g.value_type));
            }
        }
        for d in defs {
            let table = if d.global { &mut self.globals } else { &mut self.scripts };
            table.insert(d.name.clone(), Some((d.index as u16, d.ty)));
        }
    }

    fn body(&mut self, d: &Def<'_>) -> Option<usize> {
        if let [one] = d.body {
            return self.expr(one, Some(d.ty), 0);
        }
        let Some(begin) = self.begin else {
            self.err(
                d.body[1].span(),
                "body.many",
                "this body has more than one expression, and no builtin of this map is named `begin` to join them",
            );
            return None;
        };
        self.builtin_call(begin, b"begin", d.span, d.body, Some(d.ty), 0)
    }

    fn expr(&mut self, n: &Syn, expected: Option<u16>, depth: usize) -> Option<usize> {
        if depth > MAX_DEPTH {
            self.err(n.span(), "call.depth", format!("calls nest at most {MAX_DEPTH} deep"));
            return None;
        }
        match n {
            Syn::List(list) => self.list(list, expected, depth),
            _ => self.atom(n, expected),
        }
    }

    fn list(&mut self, list: &List, expected: Option<u16>, depth: usize) -> Option<usize> {
        let span = (list.open, list.end);
        let Some(head) = list.items.first() else {
            self.err(span, "call.empty", "this list calls nothing: a call starts with a function name");
            return None;
        };
        if head.list().is_some() {
            self.err(head.span(), "call.head", "a call starts with a function name, not a list");
            return None;
        }
        let token = self.spelling(head)?;
        let name = String::from_utf8_lossy(&token).into_owned();
        let args = &list.items[1..];
        if let Some(&entry) = self.scripts.get(name.as_str()) {
            let Some((index, returns)) = entry else {
                self.err(head.span(), "call.ambiguous", format!("the map has more than one script `{name}`"));
                return None;
            };
            if !args.is_empty() {
                self.err(span, "call.script_arguments", format!("`{name}` is a script, and scripts take no arguments"));
                return None;
            }
            let ty = self.check(expected, returns, span, &name)?;
            return Some(self.call(index, SCRIPT_CALL, ty, &token, Vec::new()));
        }
        let index = self.builtin(&name, head.span())?;
        self.builtin_call(index, &token, span, args, expected, depth)
    }

    fn builtin(&mut self, name: &str, span: (usize, usize)) -> Option<u16> {
        let count = self.catalog.functions().len();
        if let Some(rest) = name.strip_prefix("#builtin:") {
            return match rest.parse::<u16>() {
                Ok(i) if usize::from(i) < count => Some(i),
                _ => {
                    self.err(span, "call.unknown", format!("`{name}` is not a builtin of this executable (0 to {})", count.saturating_sub(1)));
                    None
                }
            };
        }
        match self.builtins.get(name) {
            Some(Some(i)) => Some(*i),
            Some(None) => {
                self.err(span, "call.ambiguous", format!("`{name}` names more than one builtin"));
                None
            }
            None => {
                self.err(span, "call.unknown", format!("nothing is named `{name}`: no script of this map and no named builtin has that name"));
                None
            }
        }
    }

    fn builtin_call(
        &mut self,
        index: u16,
        written: &[u8],
        span: (usize, usize),
        args: &[Syn],
        expected: Option<u16>,
        depth: usize,
    ) -> Option<usize> {
        let f = self.catalog.function(index).ok()?;
        let (returns, flags, params) = (f.return_type(), f.flags(), f.parameters().to_vec());
        let label = self.label(index);
        let ty = self.check(expected, returns, span, &label);
        let mut children = Vec::with_capacity(args.len());
        let mut ok = ty.is_some();
        if flags & 2 == 0 {
            if args.len() != params.len() {
                let wants: Vec<String> = params.iter().map(|&p| format!("<{}>", self.type_name(p))).collect();
                self.err(
                    span,
                    "call.arity",
                    format!(
                        "`{label}` takes {} argument{} {}, not {}",
                        params.len(),
                        if params.len() == 1 { "" } else { "s" },
                        if wants.is_empty() { "(none)".to_owned() } else { wants.join(" ") },
                        args.len()
                    ),
                );
                return None;
            }
            for (a, &p) in args.iter().zip(&params) {
                match self.expr(a, Some(p), depth + 1) {
                    Some(c) => children.push(c),
                    None => ok = false,
                }
            }
        } else {
            let parent = ty.unwrap_or(returns);
            for (k, a) in args.iter().enumerate() {
                let expected = self.rule(index, k, args.len(), parent);
                match self.expr(a, expected, depth + 1) {
                    Some(c) => children.push(c),
                    None => ok = false,
                }
            }
        }
        if !ok {
            return None;
        }
        let token = match self.named.get(usize::from(index)).cloned().flatten() {
            Some(name) => name.into_bytes(),
            None => written.to_vec(),
        };
        Some(self.call(index, CALL, ty?, &token, children))
    }

    /// The type an argument of a builtin that reads its own arguments takes.
    fn rule(&self, index: u16, k: usize, len: usize, parent: u16) -> Option<u16> {
        let order = match position(k, len) {
            Pos::Only => [Pos::Only, Pos::Last, Pos::First],
            p => [p; 3],
        };
        for p in order {
            if let Some(seen) = self.rules.get(&(index, p)) {
                return match seen.rule() {
                    Rule::Parent => Some(parent),
                    Rule::Fixed(t) => Some(t),
                    Rule::Natural => None,
                };
            }
        }
        None
    }

    /// The node type of a value of type `natural` where `expected` is wanted.
    fn check(&mut self, expected: Option<u16>, natural: u16, span: (usize, usize), what: &str) -> Option<u16> {
        match want(expected) {
            None => Some(natural),
            Some(t) if castable(t, natural) => Some(t),
            Some(t) => {
                let (from, to) = (self.type_name(natural), self.type_name(t));
                self.err(
                    span,
                    "type.cast",
                    format!("`{what}` is a {from}, and a {to} is wanted here: there is no cast from {from} to {to}"),
                );
                None
            }
        }
    }

    fn atom(&mut self, n: &Syn, expected: Option<u16>) -> Option<usize> {
        let span = n.span();
        let quoted = matches!(n, Syn::Str { .. });
        let token = self.spelling(n)?;
        let want = want(expected);
        let text = std::str::from_utf8(&token).ok().map(str::to_owned);
        if !quoted && let Some(name) = text.as_deref() {
            if let Some(&g) = self.globals.get(name) {
                let Some((index, ty)) = g else {
                    self.err(span, "name.ambiguous", format!("the map has more than one global `{name}`"));
                    return None;
                };
                let node = self.check(want, ty, span, name)?;
                return Some(self.push_atom(ty, node, GLOBAL, Some(&token), u32::from(index)));
            }
            if let Some(&x) = self.externals.get(name) {
                let Some((data, ty)) = x else {
                    self.err(span, "name.ambiguous", format!("the map's scripts use `{name}` for more than one game global"));
                    return None;
                };
                let node = self.check(want, ty, span, name)?;
                return Some(self.push_atom(ty, node, GLOBAL, Some(&token), data));
            }
        }
        if let Some(t) = want
            && t >= SCRIPT
            && let Some((source, data)) = self.symbol(t, &token, text.as_deref())
        {
            return Some(self.push_atom(source, t, ATOM, Some(&token), data));
        }
        let shown = String::from_utf8_lossy(&token).into_owned();
        match self.natural(&token, quoted, want, text.as_deref()) {
            Ok(lit) => {
                let node = self.check(want, lit.source, span, &shown)?;
                let keep = lit.token.then_some(&token[..]);
                Some(self.push_atom(lit.source, node, ATOM, keep, lit.data))
            }
            Err((code, text)) => {
                self.err(span, code, text);
                None
            }
        }
    }

    /// A name of type `t`: as the map's scripts already use it, a script,
    /// or what the caller's resolver finds.
    fn symbol(&self, t: u16, token: &[u8], text: Option<&str>) -> Option<(u16, u32)> {
        if let Some(&Some(v)) = self.by_value.get(&(t, token.to_vec())) {
            return Some(v);
        }
        if let Some(&Some(d)) = self.by_source.get(&(t, token.to_vec())) {
            return Some((t, d));
        }
        if t == SCRIPT
            && let Some(name) = text
            && let Some(&Some((i, _))) = self.scripts.get(name)
        {
            return Some((SCRIPT, self.script_high | u32::from(i)));
        }
        let resolve = self.resolve?;
        resolve(t, &self.type_name(t), token).map(|d| (t, d))
    }

    /// A literal's own type, from its form: `true`, `false`, `none`, a
    /// number, a quoted string, a script, or a name the map's scripts use
    /// with one type.
    fn natural(
        &self,
        token: &[u8],
        quoted: bool,
        want: Option<u16>,
        text: Option<&str>,
    ) -> std::result::Result<Lit, (&'static str, String)> {
        let lit = |source, data, token| Ok(Lit { source, data, token });
        let shown = String::from_utf8_lossy(token);
        let unresolved = |t: u16| {
            (
                "name.unresolved",
                format!(
                    "no {} is named `{shown}`: the map's scripts do not use it and nothing resolves it",
                    self.type_name(t)
                ),
            )
        };
        if quoted {
            return match want {
                Some(t) if t >= SCRIPT => Err(unresolved(t)),
                _ => lit(types::STRING, 0, true),
            };
        }
        let Some(text) = text else {
            return Err(("name.unknown", format!("`{shown}` is not UTF-8 text")));
        };
        match text {
            "true" => return lit(types::BOOLEAN, 1, false),
            "false" => return lit(types::BOOLEAN, 0, false),
            "none" => return lit(types::VOID, 0, false),
            _ => {}
        }
        if let Some(hex) = text.strip_prefix("#real-bits:") {
            return match u32::from_str_radix(hex, 16) {
                Ok(bits) if hex.len() == 8 => lit(types::REAL, bits, false),
                _ => Err(("value.number", format!("`{text}` is not `#real-bits:` and eight hex digits"))),
            };
        }
        if let Ok(v) = text.parse::<i64>() {
            let range = |name: &str| ("value.range", format!("`{text}` is out of a {name}'s range"));
            return match want {
                Some(types::LONG) => i32::try_from(v).map_or_else(|_| Err(range("long")), |v| lit(types::LONG, v as u32, false)),
                Some(types::SHORT) => {
                    i16::try_from(v).map_or_else(|_| Err(range("short")), |v| lit(types::SHORT, u32::from(v as u16), false))
                }
                _ => match (i16::try_from(v), i32::try_from(v)) {
                    (Ok(v), _) => lit(types::SHORT, u32::from(v as u16), false),
                    (_, Ok(v)) => lit(types::LONG, v as u32, false),
                    _ => Err(range("long")),
                },
            };
        }
        let digits = text.strip_prefix('-').unwrap_or(text);
        if digits.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
            return match text.parse::<f32>() {
                Ok(v) if v.is_finite() => lit(types::REAL, v.to_bits(), false),
                Ok(_) => Err(("value.range", format!("`{text}` is out of a real's range"))),
                Err(_) => Err(("value.number", format!("`{text}` is not a number"))),
            };
        }
        if let Some(&Some((i, _))) = self.scripts.get(text) {
            return lit(SCRIPT, self.script_high | u32::from(i), true);
        }
        if let Some(&Some((source, data))) = self.by_spelling.get(token) {
            return lit(source, data, true);
        }
        if want == Some(types::STRING) {
            return lit(types::STRING, 0, true);
        }
        Err(match want {
            Some(t) if t >= SCRIPT => unresolved(t),
            Some(t) => ("value.type", format!("`{text}` is not a {}", self.type_name(t))),
            None => (
                "name.unknown",
                format!("nothing is named `{text}`, and its type cannot be told here"),
            ),
        })
    }

    fn intern(&mut self, token: &[u8]) -> u32 {
        if let Some(&at) = self.interned.get(token) {
            return at;
        }
        let at = self.strings.len() as u32;
        self.strings.extend_from_slice(token);
        self.strings.push(0);
        self.interned.insert(token.to_vec(), at);
        at
    }

    fn push(&mut self, n: New) -> usize {
        self.arena.push(n);
        self.arena.len() - 1
    }

    fn push_atom(&mut self, source: u16, ty: u16, flags: u16, token: Option<&[u8]>, data: u32) -> usize {
        let string = token.map_or(u32::MAX, |t| self.intern(t));
        // A string literal's data is its own offset until the map loads.
        let data = if flags == ATOM && source == types::STRING { string } else { data };
        self.push(New {
            opcode: source,
            ty,
            flags,
            string,
            data,
            child: None,
            next: None,
        })
    }

    fn call(&mut self, opcode: u16, flags: u16, ty: u16, token: &[u8], children: Vec<usize>) -> usize {
        let string = self.intern(token);
        let head = self.push(New {
            opcode,
            ty: FUNCTION_NAME,
            flags: ATOM,
            string,
            data: 0,
            child: None,
            next: children.first().copied(),
        });
        for pair in children.windows(2) {
            self.arena[pair[0]].next = Some(pair[1]);
        }
        self.push(New {
            opcode,
            ty,
            flags,
            string: u32::MAX,
            data: 0,
            child: Some(head),
            next: None,
        })
    }

    /// Free the replaced definitions' nodes, place the new ones, write the
    /// tables and check them.
    fn finish(&mut self, defs: &[Def<'_>], roots: &[usize]) -> std::result::Result<Compiled, Raw> {
        let s = self.scenario;
        let here = defs.first().map_or((0, 0), |d| d.span);
        let raw = |code, text: String| Raw {
            start: here.0,
            end: here.1,
            code,
            text,
        };
        let gone_scripts: BTreeSet<usize> = defs.iter().filter(|d| !d.global && d.replaces).map(|d| d.index).collect();
        let gone_globals: BTreeSet<usize> = defs.iter().filter(|d| d.global && d.replaces).map(|d| d.index).collect();
        let (mut gone, mut kept) = (Vec::new(), Vec::new());
        for (i, x) in s.scripts.iter().enumerate() {
            let to = if gone_scripts.contains(&i) { &mut gone } else { &mut kept };
            to.push(x.root);
        }
        for (i, x) in s.globals.iter().enumerate() {
            let to = if gone_globals.contains(&i) { &mut gone } else { &mut kept };
            to.push(x.root);
        }
        let kept = structural(s, &kept);
        let freed: Vec<usize> = structural(s, &gone).difference(&kept).copied().collect();
        let mut nodes = s.nodes.clone();
        for &slot in &freed {
            nodes[slot].salt = 0;
        }
        let free: Vec<usize> = (0..nodes.len()).filter(|&i| nodes[i].salt == 0).collect();
        let needed = self.arena.len();
        let grown = needed.saturating_sub(free.len());
        let limit = self
            .node_limit
            .unwrap_or(RETAIL_NODES)
            .max(s.nodes.len())
            .min(MAX_NODES);
        if nodes.len() + grown > limit {
            return Err(raw(
                "nodes.limit",
                format!(
                    "the scripts need {} node slots, and this map holds at most {limit}",
                    nodes.len() + grown
                ),
            ));
        }
        if self.strings.len() > MAX_STRING_BYTES {
            return Err(raw(
                "strings.limit",
                format!("the script strings would be {} bytes; at most {MAX_STRING_BYTES}", self.strings.len()),
            ));
        }
        let mut salt = s.nodes.iter().map(|n| n.salt).max().unwrap_or(0);
        let mut free = free.into_iter();
        let mut datums = Vec::with_capacity(needed);
        for _ in 0..needed {
            let slot = free.next().unwrap_or_else(|| {
                nodes.push(Node {
                    salt: 0,
                    opcode: 0,
                    value_type: 0,
                    flags: 0,
                    next: Datum::NULL,
                    string_offset: 0,
                    data: 0,
                });
                nodes.len() - 1
            });
            salt = salt.wrapping_add(1) | 0x8000;
            datums.push(Datum((u32::from(salt) << 16) | slot as u32));
        }
        for (new, &datum) in self.arena.iter().zip(&datums) {
            nodes[datum.slot()] = Node {
                salt: datum.salt(),
                opcode: new.opcode,
                value_type: new.ty,
                flags: new.flags,
                next: new.next.map_or(Datum::NULL, |i| datums[i]),
                string_offset: new.string,
                data: new.child.map_or(new.data, |i| datums[i].0),
            };
        }
        let mut scripts = s.scripts.clone();
        let mut globals = s.globals.clone();
        let mut items = Vec::with_capacity(defs.len());
        for (d, &root) in defs.iter().zip(roots) {
            let root = datums[root];
            if d.global {
                items.push(Item::Global(d.index));
                if d.replaces {
                    let g = &mut globals[d.index];
                    g.value_type = d.ty;
                    g.root = root;
                } else {
                    globals.push(Global {
                        name_bytes: fixed_name(&d.name),
                        value_type: d.ty,
                        reserved: 0,
                        root,
                    });
                }
            } else {
                items.push(Item::Script(d.index));
                if d.replaces {
                    let x = &mut scripts[d.index];
                    x.kind = d.kind;
                    x.return_type = d.ty;
                    x.root = root;
                } else {
                    scripts.push(Script {
                        name_bytes: fixed_name(&d.name),
                        kind: d.kind,
                        return_type: d.ty,
                        root,
                    });
                }
            }
        }
        let new_strings = self.strings.len() - s.strings.len();
        let scenario = Scenario {
            nodes,
            strings: std::mem::take(&mut self.strings),
            scripts,
            globals,
            references: s.references.clone(),
        };
        scenario
            .validate(self.catalog)
            .map_err(|e| raw("compile.check", format!("the compiled tables do not check: {e}")))?;
        Ok(Compiled {
            scenario,
            items,
            nodes: needed,
            freed: freed.len(),
            reused: needed - grown,
            grown,
            new_strings,
        })
    }
}

/// Slots reached from `roots` through calls and argument lists, not into
/// called scripts.
fn structural(s: &Scenario, roots: &[Datum]) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut stack = roots.to_vec();
    while let Some(d) = stack.pop() {
        if d == Datum::NULL {
            continue;
        }
        let Ok(n) = s.node(d) else { continue };
        if !seen.insert(d.slot()) {
            continue;
        }
        stack.push(n.next);
        if !n.atom() {
            stack.push(Datum(n.data));
        }
    }
    seen
}

fn fixed_name(name: &str) -> [u8; 32] {
    let mut b = [0; 32];
    b[..name.len()].copy_from_slice(name.as_bytes());
    b
}

/// A quoted string's bytes: `\"`, `\\`, `\n`, `\r`, `\t`, `\xNN` and
/// `\u{N}`, the decompiler's escapes.
fn unescape(raw: &str) -> std::result::Result<Vec<u8>, String> {
    let inner = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or("this string has no closing quote")?;
    let b = inner.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        match b.get(i + 1) {
            Some(b'"') => out.push(b'"'),
            Some(b'\\') => out.push(b'\\'),
            Some(b'n') => out.push(b'\n'),
            Some(b'r') => out.push(b'\r'),
            Some(b't') => out.push(b'\t'),
            Some(b'x') => {
                let v = inner
                    .get(i + 2..i + 4)
                    .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or("`\\x` takes two hex digits")?;
                out.push(v);
                i += 4;
                continue;
            }
            Some(b'u') => {
                let rest = inner.get(i + 2..).unwrap_or("");
                let close = rest.find('}').filter(|_| rest.starts_with('{')).ok_or("`\\u` takes `{hex}`")?;
                let c = u32::from_str_radix(&rest[1..close], 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or("`\\u{...}` is not a character")?;
                let mut buf = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                i += 2 + close + 1;
                continue;
            }
            _ => {
                return Err(
                    "this is not an escape: strings take \\\" \\\\ \\n \\r \\t \\xNN and \\u{N}".to_owned(),
                );
            }
        }
        i += 2;
    }
    if out.contains(&0) {
        return Err("a script string cannot hold a NUL byte".to_owned());
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests;
