//! Halo 1 scripts as readable HSC text, with every node checked on the way.
//!
//! Each call node and its function-name atom become one list, as Halo 2's
//! renderer in `crates/formats/hs` does. Literals render from their bits by source
//! type (the atom's `+2` word); symbolic values (object names, AI, tags,
//! enums) render as the token the compiler kept in the string data.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;

use hs::{Datum, Node};

use crate::scripts::catalog::{Catalog, types};
use crate::scripts::layout::Tables;
use crate::scripts::{Result, invalid};

/// Deepest expression nesting accepted (host limit).
pub const MAX_DEPTH: usize = 512;

/// What one scenario's scripts hold, counted while decompiling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Scripts.
    pub scripts: usize,
    /// Globals.
    pub globals: usize,
    /// References.
    pub references: usize,
    /// Node slots with a non-zero salt.
    pub live_nodes: usize,
    /// Distinct live nodes reached from a script or global root.
    pub reached_nodes: usize,
    /// Node occurrences written to the source (a shared node counts each
    /// time it is written).
    pub emitted_nodes: usize,
    /// Calls to builtins.
    pub builtin_calls: usize,
    /// Calls with a fixed signature, whose argument types were checked
    /// against the declared parameters.
    pub checked_calls: usize,
    /// Calls to special forms (own parser, no fixed signature).
    pub special_calls: usize,
    /// Calls to scenario scripts.
    pub script_calls: usize,
    /// References to globals.
    pub global_refs: usize,
    /// Literal atoms.
    pub literals: usize,
    /// Calls whose node type differs from the builtin's return type: the
    /// interpreter casts the result.
    pub casts: usize,
    /// Call heads whose index differs from their call's.
    pub stale_heads: usize,
    /// Uses of each builtin, by index.
    pub builtin_uses: BTreeMap<u16, usize>,
}

/// Source text and the counts behind it.
#[derive(Debug, Clone)]
pub struct Decompiled {
    /// HSC text: one `(global ...)` per global, then one `(script ...)` per
    /// script.
    pub source: String,
    /// Counts.
    pub stats: Stats,
}

/// Decompiles every global and script, checking each node: no stale links,
/// no unknown builtin, head names that match the catalog, argument types that
/// match fixed signatures, script and global indices that match names.
pub fn decompile(t: &Tables, c: &Catalog) -> Result<Decompiled> {
    let mut r = Renderer {
        t,
        c,
        out: String::new(),
        stats: Stats {
            scripts: t.scripts.len(),
            globals: t.globals.len(),
            references: t.references.len(),
            live_nodes: t.nodes.iter().filter(|n| n.live()).count(),
            ..Stats::default()
        },
        reached: HashSet::new(),
    };
    for g in &t.globals {
        let ty = c.type_name(g.value_type)?;
        write!(r.out, "(global {ty} {} ", g.name).unwrap();
        r.expr(g.root, 0)?;
        r.out.push_str(")\n\n");
    }
    for s in &t.scripts {
        let kind = c.script_kind(s.kind)?;
        r.out.push_str("(script ");
        r.out.push_str(kind);
        if matches!(kind, "static" | "stub") {
            write!(r.out, " {}", c.type_name(s.return_type)?).unwrap();
        }
        write!(r.out, " {}", s.name).unwrap();
        // A script's root is an implicit `begin`; its body is the arguments.
        r.indent(1);
        r.expr(s.root, 1)?;
        r.out.push_str(")\n\n");
    }
    r.stats.reached_nodes = r.reached.len();
    Ok(Decompiled {
        source: r.out,
        stats: r.stats,
    })
}

struct Renderer<'a> {
    t: &'a Tables,
    c: &'a Catalog,
    out: String,
    stats: Stats,
    reached: HashSet<usize>,
}

impl Renderer<'_> {
    fn indent(&mut self, depth: usize) {
        self.out.push('\n');
        for _ in 0..depth {
            self.out.push_str("    ");
        }
    }

    fn node(&mut self, d: Datum) -> Result<Node> {
        let n = *self.t.node(d)?;
        self.c.type_name(n.value_type())?;
        self.reached.insert(d.slot());
        self.stats.emitted_nodes += 1;
        Ok(n)
    }

    fn expr(&mut self, d: Datum, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(invalid("expression nested too deep"));
        }
        let n = self.node(d)?;
        if n.atom() {
            return self.atom(&n);
        }
        let head = self.node(Datum(n.data()))?;
        if !head.atom() || head.global() || head.value_type() != types::FUNCTION_NAME {
            return Err(invalid(format!("node {:#x}: bad call head", d.0)));
        }
        // The call's index is authoritative; some heads keep an index one
        // lower than it (see the format doc), and the name check below catches
        // a call whose index and name disagree.
        if head.opcode() != n.opcode() {
            if head.opcode().wrapping_add(1) != n.opcode() {
                return Err(invalid(format!(
                    "node {:#x}: call {} has head {}",
                    d.0,
                    n.opcode(),
                    head.opcode()
                )));
            }
            self.stats.stale_heads += 1;
        }
        let token = self.t.string(head.string_offset())?;
        let args = self.args(head.next())?;
        if n.script_call() {
            let s = self
                .t
                .scripts
                .get(n.opcode() as usize)
                .ok_or_else(|| invalid(format!("script call {} out of range", n.opcode())))?;
            if s.name != token {
                return Err(invalid(format!(
                    "script call {token}: index names {}",
                    s.name
                )));
            }
            if !args.is_empty() {
                return Err(invalid(format!("script call {token} has arguments")));
            }
            self.stats.script_calls += 1;
        } else {
            let f = self.c.function(n.opcode())?;
            if f.name != token {
                return Err(invalid(format!(
                    "builtin {} is {}, head says {token}",
                    n.opcode(),
                    f.name
                )));
            }
            self.stats.builtin_calls += 1;
            *self.stats.builtin_uses.entry(n.opcode()).or_default() += 1;
            if n.value_type() != f.return_type {
                self.stats.casts += 1;
            }
            if self.c.fixed_signature(n.opcode()) {
                if args.len() != f.parameters.len() {
                    return Err(invalid(format!(
                        "{token}: {} arguments, {} parameters",
                        args.len(),
                        f.parameters.len()
                    )));
                }
                for (&a, &p) in args.iter().zip(&f.parameters) {
                    let ty = self.t.node(a)?.value_type();
                    if ty != p {
                        return Err(invalid(format!(
                            "{token}: argument type {} for parameter {}",
                            self.c.type_name(ty)?,
                            self.c.type_name(p)?
                        )));
                    }
                }
                self.stats.checked_calls += 1;
            } else {
                self.stats.special_calls += 1;
            }
        }
        self.out.push('(');
        self.out.push_str(token);
        let block = args.len() > 2 || matches!(token, "begin" | "if" | "cond" | "sleep_until");
        for a in args {
            if block {
                self.indent(depth + 1);
            } else {
                self.out.push(' ');
            }
            self.expr(a, depth + 1)?;
        }
        self.out.push(')');
        Ok(())
    }

    fn args(&self, mut next: Datum) -> Result<Vec<Datum>> {
        let mut args = Vec::new();
        while next != Datum::NULL {
            if args.len() > self.t.nodes.len() {
                return Err(invalid("cyclic argument list"));
            }
            args.push(next);
            next = self.t.node(next)?.next();
        }
        Ok(args)
    }

    fn atom(&mut self, n: &Node) -> Result<()> {
        if n.global() {
            let index = n.data() as u16;
            let token = self.t.string(n.string_offset())?;
            if index & 0x8000 == 0 {
                let g = self
                    .t
                    .globals
                    .get(index as usize)
                    .ok_or_else(|| invalid(format!("global {index} out of range")))?;
                if g.name != token {
                    return Err(invalid(format!("global {token}: index names {}", g.name)));
                }
            }
            self.stats.global_refs += 1;
            self.out.push_str(token);
            return Ok(());
        }
        if n.value_type() == types::FUNCTION_NAME {
            return Err(invalid("function name outside a call head"));
        }
        self.stats.literals += 1;
        let text = literal(self.t, self.c, n)?;
        self.out.push_str(&text);
        Ok(())
    }
}

/// A literal atom as source text, by its source type (`+2`).
pub fn literal(t: &Tables, c: &Catalog, n: &Node) -> Result<String> {
    let bits = n.data();
    Ok(match c.type_name(n.opcode())? {
        _ if n.opcode() == types::VOID => "none".to_owned(),
        _ if n.opcode() == types::BOOLEAN => {
            (if bits as u8 != 0 { "true" } else { "false" }).into()
        }
        _ if n.opcode() == types::REAL => {
            let v = f32::from_bits(bits);
            if v.is_finite() {
                format!("{v:?}")
            } else {
                format!("#real-bits:{bits:08x}")
            }
        }
        _ if n.opcode() == types::SHORT => (bits as i16).to_string(),
        _ if n.opcode() == types::LONG => (bits as i32).to_string(),
        _ if n.opcode() == types::STRING => quote(t.string_bytes(n.string_offset())?),
        _ => {
            let s = t.string(n.string_offset())?;
            if s.is_empty() || s.contains(char::is_whitespace) {
                format!("\"{s}\"")
            } else {
                s.to_owned()
            }
        }
    })
}

/// A string literal in quotes: `"` and `\\` escaped, bytes outside printable
/// ASCII as `\xNN`.
pub fn quote(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b' '..=b'~' => out.push(b as char),
            _ => write!(out, "\\x{b:02x}").unwrap(),
        }
    }
    out.push('"');
    out
}
