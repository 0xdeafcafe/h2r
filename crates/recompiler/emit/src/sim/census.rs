//! Every write a tick can make, by where it lands: a static pass over the
//! lifted IR of a [`Closure`] (the tick's, or the whole image's).
//!
//! The sim tier's memory contract (one contiguous mapping, no dirty
//! marking) holds only if the tick's writes stay in memory the tier owns.
//! This pass lists every store and classifies its address:
//!
//! - [`Kind::Absolute`]: a constant address (a global), with its section.
//! - [`Kind::Indexed`]: a constant base plus a computed index (a global
//!   array).
//! - [`Kind::Field`]: an offset from a pointer the function was handed or
//!   loaded once: from a global, a stack argument or an entry register.
//! - [`Kind::Reachable`]: deeper in an object graph rooted at one of those.
//! - [`Kind::Stack`]: the thread's stack.
//! - [`Kind::Fs`]: thread-local storage.
//! - [`Kind::Unknown`]: nothing the pass can name.
//!
//! The arena is allocated at run time, so whether a `Field` or `Reachable`
//! store lands inside it is a run-time question; this pass names the roots
//! to check. `docs/engine/sim-tier-writes.md` explains the method and
//! holds the output.
//!
//! The pass is an abstract interpretation per function, joined at blocks.
//! After a direct call, the registers the callee's recovered signature may
//! write ([`super::Signature::writes`]) become unknown; ESP moves by the
//! return address and the callee's `ret n`. Indirect calls and jumps are
//! listed, not followed: their callees are outside the closure's coverage.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use ir::{BinOp, Control, Expr, Gpr, IrFunction, Segment, Slice, Stmt, Target, Value, Width};

use super::{Closure, Signature};

/// What a pointer is rooted at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Root {
    /// The value stored at this absolute address.
    Global(u32),
    /// The stack argument at this offset from ESP at entry (4 is the first).
    Arg(i32),
    /// The register's value at entry ([`Gpr::index`]).
    Reg(u8),
}

impl std::fmt::Display for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Root::Global(g) => write!(f, "[{g:#010x}]"),
            Root::Arg(o) => write!(f, "arg[esp+{o:#x}]"),
            Root::Reg(r) => write!(f, "entry {}", super::REG[usize::from(*r & 7)]),
        }
    }
}

/// The abstract value of a register or temp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abs {
    Unknown,
    Const(u32),
    /// ESP at entry plus an offset.
    Stack(Option<i32>),
    /// A root's value plus an offset.
    At(Root, Option<i32>),
    /// A constant base plus something computed.
    Near(u32),
    /// Loaded through one or more pointers from a root.
    Under(Root),
}

fn join(a: Abs, b: Abs) -> Abs {
    use Abs::*;
    match (a, b) {
        _ if a == b => a,
        (Stack(_), Stack(_)) => Stack(None),
        (At(r, _), At(q, _)) if r == q => At(r, None),
        (Near(x), Near(y)) if x == y => Near(x),
        (Under(r), Under(q)) | (At(r, _), Under(q)) | (Under(r), At(q, _)) if r == q => Under(r),
        _ => Unknown,
    }
}

/// After a loop: a value the body changed has lost its offset.
fn widen(a: Abs) -> Abs {
    match a {
        Abs::Const(c) => Abs::Near(c),
        Abs::Stack(_) => Abs::Stack(None),
        Abs::At(r, _) => Abs::At(r, None),
        a => a,
    }
}

fn plus(a: Abs, b: Abs, sub: bool) -> Abs {
    use Abs::*;
    let off = |o: Option<i32>, c: u32| o.map(|o| if sub { o.wrapping_sub(c as i32) } else { o.wrapping_add(c as i32) });
    match (a, b) {
        (Const(x), Const(y)) => Const(if sub { x.wrapping_sub(y) } else { x.wrapping_add(y) }),
        (Stack(o), Const(c)) => Stack(off(o, c)),
        (Const(c), Stack(o)) if !sub => Stack(off(o, c)),
        (Stack(_), _) if !sub => Stack(None),
        (_, Stack(_)) if !sub => Stack(None),
        (At(r, o), Const(c)) => At(r, off(o, c)),
        (Const(c), At(r, o)) if !sub => At(r, off(o, c)),
        (At(r, _), _) | (_, At(r, _)) if !sub => At(r, None),
        (Under(r), _) | (_, Under(r)) if !sub => Under(r),
        (Near(x), _) | (_, Near(x)) if !sub => Near(x),
        // A constant base indexed by something computed: a global array.
        (Const(c), _) | (_, Const(c)) if !sub && c >= 0x1_0000 => Near(c),
        _ => Unknown,
    }
}

/// Where a store lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Absolute(u32),
    Indexed(u32),
    Field(Root, Option<i32>),
    Reachable(Root),
    Stack,
    Fs(Option<u32>),
    Unknown,
}

/// One store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    pub function: u32,
    pub pc: u32,
    pub kind: Kind,
    /// Bytes per store.
    pub bytes: u32,
    /// Inside a `rep` loop: the store repeats over a range.
    pub repeat: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct State {
    regs: [Abs; 8],
}

impl State {
    fn entry() -> State {
        let mut regs = [Abs::Unknown; 8];
        for g in Gpr::ALL {
            regs[g.index()] = Abs::At(Root::Reg(g.index() as u8), Some(0));
        }
        regs[Gpr::Esp.index()] = Abs::Stack(Some(0));
        State { regs }
    }

    fn join(&self, o: &State) -> State {
        let mut regs = self.regs;
        for (r, q) in regs.iter_mut().zip(o.regs) {
            *r = join(*r, q);
        }
        State { regs }
    }
}

struct Walk<'a> {
    f: &'a IrFunction,
    temps: BTreeMap<u32, Abs>,
    sites: Vec<Site>,
}

impl Walk<'_> {
    fn value(&self, v: &Value) -> Abs {
        match v {
            Value::Const { bits, .. } => Abs::Const(*bits as u32),
            Value::Temp(t) => self.temps.get(&t.id).copied().unwrap_or(Abs::Unknown),
        }
    }

    fn expr(&self, w: Width, e: &Expr) -> Abs {
        if w == Width::W32
            && let Some(c) = e.fold(w)
        {
            return Abs::Const(c as u32);
        }
        match e {
            Expr::Binary { op: BinOp::Add, a, b } => plus(self.value(a), self.value(b), false),
            Expr::Binary { op: BinOp::Sub, a, b } => plus(self.value(a), self.value(b), true),
            Expr::Select { a, b, .. } => join(self.value(a), self.value(b)),
            _ => Abs::Unknown,
        }
    }

    fn load(&self, w: Width, addr: Abs) -> Abs {
        if w != Width::W32 {
            return Abs::Unknown;
        }
        match addr {
            Abs::Const(a) => Abs::At(Root::Global(a), Some(0)),
            Abs::Stack(Some(o)) if o >= 4 => Abs::At(Root::Arg(o), Some(0)),
            Abs::At(r, _) | Abs::Under(r) => Abs::Under(r),
            Abs::Near(a) => Abs::Under(Root::Global(a)),
            _ => Abs::Unknown,
        }
    }

    fn store(&mut self, pc: u32, seg: Segment, addr: Abs, bytes: u32, repeat: bool) {
        let kind = match (seg, addr) {
            (Segment::Fs, Abs::Const(o)) => Kind::Fs(Some(o)),
            (Segment::Fs, _) => Kind::Fs(None),
            (_, Abs::Const(a)) => Kind::Absolute(a),
            (_, Abs::Near(a)) => Kind::Indexed(a),
            (_, Abs::Stack(_)) => Kind::Stack,
            (_, Abs::At(r, o)) => Kind::Field(r, o),
            (_, Abs::Under(r)) => Kind::Reachable(r),
            (_, Abs::Unknown) => Kind::Unknown,
        };
        self.sites.push(Site { function: self.f.entry, pc, kind, bytes, repeat });
    }

    fn stmts(&mut self, list: &[Stmt], st: &mut State, pc: u32, repeat: bool) {
        for s in list {
            match s {
                Stmt::Assign { dst, expr } => {
                    let v = self.expr(dst.width, expr);
                    self.temps.insert(dst.id, v);
                }
                Stmt::ReadReg { dst, reg, slice } => {
                    let v = if *slice == Slice::Full { st.regs[reg.index()] } else { Abs::Unknown };
                    self.temps.insert(dst.id, v);
                }
                Stmt::WriteReg { reg, slice, value } => {
                    st.regs[reg.index()] = if *slice == Slice::Full { self.value(value) } else { Abs::Unknown };
                }
                Stmt::Load { dst, addr } => {
                    let a = self.value(&addr.offset);
                    let v = if addr.seg == Segment::Flat { self.load(dst.width, a) } else { Abs::Unknown };
                    self.temps.insert(dst.id, v);
                }
                Stmt::Store { addr, value } => {
                    let a = self.value(&addr.offset);
                    self.store(pc, addr.seg, a, value.width().bytes(), repeat);
                }
                Stmt::Intrinsic { dsts, .. } => {
                    for t in dsts {
                        self.temps.insert(t.id, Abs::Unknown);
                    }
                }
                Stmt::ReadFlag { dst, .. } | Stmt::ReadEflags { dst } | Stmt::ReadXmm { dst, .. } | Stmt::ReadMmx { dst, .. } => {
                    self.temps.insert(dst.id, Abs::Unknown);
                }
                Stmt::If { then, otherwise, .. } => {
                    let mut a = st.clone();
                    let mut b = st.clone();
                    self.stmts(then, &mut a, pc, repeat);
                    self.stmts(otherwise, &mut b, pc, repeat);
                    *st = a.join(&b);
                }
                Stmt::Repeat { body, .. } => {
                    let before = st.clone();
                    self.stmts(body, st, pc, true);
                    for (r, b) in st.regs.iter_mut().zip(before.regs) {
                        if *r != b {
                            *r = widen(b);
                        }
                    }
                    st.regs[Gpr::Ecx.index()] = Abs::Unknown;
                }
                _ => {}
            }
        }
    }
}

/// Every store in `f`. `sigs` are the closure's recovered signatures: they
/// say what a call leaves alone.
pub fn function_sites(f: &IrFunction, sigs: &BTreeMap<u32, Signature>) -> Vec<Site> {
    let mut w = Walk { f, temps: BTreeMap::new(), sites: Vec::new() };
    let mut input: BTreeMap<u32, State> = BTreeMap::from([(f.entry, State::entry())]);
    let mut work = std::collections::VecDeque::from([f.entry]);
    let mut visits: BTreeMap<u32, u32> = BTreeMap::new();
    let mut done: BTreeMap<u32, Vec<Site>> = BTreeMap::new();
    while let Some(pc) = work.pop_front() {
        let Some(b) = f.blocks.get(&pc) else { continue };
        let n = visits.entry(pc).or_default();
        *n += 1;
        let mut st = input[&pc].clone();
        // A block keeps being widened until its input settles; past a few
        // visits every value that still moves is joined to unknown.
        if *n > 8 {
            st = st.join(&State { regs: [Abs::Unknown; 8] });
        }
        w.sites.clear();
        w.temps.clear();
        for i in &b.insns {
            w.stmts(&i.stmts, &mut st, i.pc, false);
        }
        done.insert(pc, std::mem::take(&mut w.sites));
        let mut next: Vec<(u32, State)> = crate::backedge::successors(b, &f.blocks).into_iter().map(|s| (s, st.clone())).collect();
        if let Some(Control::Call { target, continuation, .. }) = b.control()
            && f.blocks.contains_key(continuation)
        {
            let sig = match target {
                Target::Direct(t) => sigs.get(t),
                Target::Dynamic(_) => None,
            };
            let mut after = st.clone();
            let writes = sig.map_or(super::ALL, |s| s.writes);
            let esp = after.regs[Gpr::Esp.index()];
            for g in Gpr::ALL {
                if writes & (1 << g.index()) != 0 {
                    after.regs[g.index()] = Abs::Unknown;
                }
            }
            after.regs[Gpr::Esp.index()] = match (esp, sig.and_then(|s| s.cleanup)) {
                (Abs::Stack(Some(o)), Some(c)) => Abs::Stack(Some(o + 4 + i32::from(c))),
                (Abs::Stack(_), _) => Abs::Stack(None),
                _ => Abs::Unknown,
            };
            next.push((*continuation, after));
        }
        for (s, t) in next {
            let merged = input.get(&s).map_or_else(|| t.clone(), |prev| prev.join(&t));
            if input.get(&s) != Some(&merged) {
                input.insert(s, merged);
                work.push_back(s);
            }
        }
    }
    done.into_values().flatten().collect()
}

/// A named address range (an XBE section).
#[derive(Debug, Clone)]
pub struct Region {
    pub name: String,
    pub start: u32,
    pub end: u32,
}

/// The census of a closure.
#[derive(Debug, Default)]
pub struct Census {
    pub sites: Vec<Site>,
    pub functions: usize,
    pub missing: BTreeSet<u32>,
    /// Indirect call and jump sites (switches excluded), by PC.
    pub indirect: BTreeSet<u32>,
    pub truncated: bool,
}

/// Run the pass over every function of `closure`.
pub fn census(closure: &Closure) -> Census {
    let sigs = super::signatures(closure, &BTreeMap::new());
    let mut c = Census {
        functions: closure.functions.len(),
        missing: closure.missing.clone(),
        truncated: closure.truncated,
        ..Census::default()
    };
    for f in closure.functions.values() {
        c.sites.extend(function_sites(f, &sigs));
        c.indirect.extend(super::edges(f).indirect);
    }
    c
}

fn region(regions: &[Region], a: u32) -> &str {
    regions.iter().find(|r| a >= r.start && a < r.end).map_or("(no section)", |r| r.name.as_str())
}

fn functions(sites: &[&Site]) -> String {
    let set: BTreeSet<u32> = sites.iter().map(|s| s.function).collect();
    let mut out: Vec<String> = set.iter().take(4).map(|f| format!("{f:#010x}")).collect();
    if set.len() > 4 {
        out.push(format!("+{}", set.len() - 4));
    }
    out.join(" ")
}

/// The census as markdown: `title`, counts, then every class.
pub fn markdown(title: &str, c: &Census, regions: &[Region]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "## {title}\n");
    let _ = writeln!(
        s,
        "{} functions, {} stores, {} indirect call/jump sites, {} missing targets{}.\n",
        c.functions,
        c.sites.len(),
        c.indirect.len(),
        c.missing.len(),
        if c.truncated { ", walk truncated at its limit" } else { "" }
    );
    let mut by: BTreeMap<Kind, Vec<&Site>> = BTreeMap::new();
    for site in &c.sites {
        by.entry(site.kind).or_default().push(site);
    }
    let count = |p: &dyn Fn(&Kind) -> bool| by.iter().filter(|(k, _)| p(k)).map(|(_, v)| v.len()).sum::<usize>();
    let _ = writeln!(s, "| class | stores |\n|---|---|");
    let classes: [(&str, &dyn Fn(&Kind) -> bool); 7] = [
        ("absolute", &|k| matches!(k, Kind::Absolute(_))),
        ("indexed global", &|k| matches!(k, Kind::Indexed(_))),
        ("field of a root", &|k| matches!(k, Kind::Field(..))),
        ("reachable from a root", &|k| matches!(k, Kind::Reachable(_))),
        ("stack", &|k| matches!(k, Kind::Stack)),
        ("fs", &|k| matches!(k, Kind::Fs(_))),
        ("unknown", &|k| matches!(k, Kind::Unknown)),
    ];
    for (name, p) in classes {
        let _ = writeln!(s, "| {name} | {} |", count(p));
    }

    let _ = writeln!(s, "\n### Absolute and indexed globals\n\n| address | section | kind | bytes | stores | functions |\n|---|---|---|---|---|---|");
    for (k, v) in &by {
        let (a, kind) = match k {
            Kind::Absolute(a) => (*a, "absolute"),
            Kind::Indexed(a) => (*a, "indexed"),
            _ => continue,
        };
        let bytes: BTreeSet<u32> = v.iter().map(|s| s.bytes).collect();
        let bytes: Vec<String> = bytes.iter().map(u32::to_string).collect();
        let _ = writeln!(s, "| {a:#010x} | {} | {kind} | {} | {} | {} |", region(regions, a), bytes.join(","), v.len(), functions(v));
    }

    let _ = writeln!(s, "\n### Through pointers held in globals\n\n| root | offsets | stores | functions |\n|---|---|---|---|");
    let mut roots: BTreeMap<u32, Vec<&Site>> = BTreeMap::new();
    for site in &c.sites {
        if let Kind::Field(Root::Global(g), _) | Kind::Reachable(Root::Global(g)) = site.kind {
            roots.entry(g).or_default().push(site);
        }
    }
    for (g, v) in &roots {
        let offs: BTreeSet<String> = v
            .iter()
            .map(|s| match s.kind {
                Kind::Field(_, Some(o)) => format!("{o:#x}"),
                Kind::Field(_, None) => "indexed".into(),
                _ => "deeper".into(),
            })
            .collect();
        let offs: Vec<String> = offs.into_iter().take(12).collect();
        let _ = writeln!(s, "| [{g:#010x}] ({}) | {} | {} | {} |", region(regions, *g), offs.join(" "), v.len(), functions(v));
    }

    let _ = writeln!(s, "\n### Through arguments and entry registers\n\n| function | root | stores |\n|---|---|---|");
    let mut local: BTreeMap<(u32, Root), usize> = BTreeMap::new();
    for site in &c.sites {
        if let Kind::Field(r @ (Root::Arg(_) | Root::Reg(_)), _) | Kind::Reachable(r @ (Root::Arg(_) | Root::Reg(_))) = site.kind {
            *local.entry((site.function, r)).or_default() += 1;
        }
    }
    for ((f, r), n) in &local {
        let _ = writeln!(s, "| {f:#010x} | {r} | {n} |");
    }

    let _ = writeln!(s, "\n### Thread-local and unknown\n\n| function | pc | kind | bytes | rep |\n|---|---|---|---|---|");
    for site in c.sites.iter().filter(|s| matches!(s.kind, Kind::Fs(_) | Kind::Unknown)) {
        let kind = match site.kind {
            Kind::Fs(Some(o)) => format!("fs:{o:#x}"),
            Kind::Fs(None) => "fs:?".into(),
            _ => "unknown".into(),
        };
        let _ = writeln!(s, "| {:#010x} | {:#010x} | {kind} | {} | {} |", site.function, site.pc, site.bytes, site.repeat);
    }

    if !c.indirect.is_empty() {
        let list: Vec<String> = c.indirect.iter().map(|p| format!("{p:#010x}")).collect();
        let _ = writeln!(s, "\n### Indirect call and jump sites (not followed)\n\n{}", list.join(" "));
    }
    if !c.missing.is_empty() {
        let list: Vec<String> = c.missing.iter().map(|p| format!("{p:#010x}")).collect();
        let _ = writeln!(s, "\n### Targets with no lifted function\n\n{}", list.join(" "));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_follow_the_stack_and_globals() {
        assert_eq!(plus(Abs::Stack(Some(0)), Abs::Const(8), false), Abs::Stack(Some(8)));
        assert_eq!(plus(Abs::Stack(Some(0)), Abs::Const(4), true), Abs::Stack(Some(-4)));
        assert_eq!(plus(Abs::At(Root::Global(0x50_0000), Some(0)), Abs::Const(0x10), false), Abs::At(Root::Global(0x50_0000), Some(0x10)));
        assert_eq!(plus(Abs::Const(0x50_0000), Abs::Unknown, false), Abs::Near(0x50_0000));
        assert_eq!(join(Abs::Stack(Some(4)), Abs::Stack(Some(8))), Abs::Stack(None));
        assert_eq!(join(Abs::Const(1), Abs::Const(2)), Abs::Unknown);
    }
}
