//! IR-to-IR optimisation passes, run between lifting and emission.
//!
//! Every pass preserves the IR's semantics at every point where state can
//! be observed: each exit from a handler (transfers, calls, returns,
//! faults, stops, preemption polls) sees exactly the registers, flags,
//! defined mask and memory the unoptimised IR would give it. Between those
//! points a pass may change anything, which is where the savings are.
//!
//! What a pass needs to know beyond the function is its [`Shape`]: where
//! the handler can be entered (start selectors, including REP restart
//! points in the middle of a block) and which blocks poll for preemption
//! on entry. The emitter's plan decides both.
//!
//! Each pass is separable and named ([`Passes`]), so a bad one can be
//! turned off without touching the others: `H2_PASSES=none`, or
//! `H2_PASSES=all,-dead-flags`.

use std::collections::BTreeSet;

use ir::IrFunction;

pub mod dce;
pub mod flags;
pub mod lazy;

/// Where a handler can be entered and where it polls.
#[derive(Debug, Clone, Copy)]
pub struct Shape<'a> {
    /// Start selectors: the entry, interior entries, call continuations,
    /// REP restart points and loop resumes the handler accepts.
    pub selectors: &'a BTreeSet<u32>,
    /// Blocks that begin with a preemption poll (loop headers).
    pub polled: &'a BTreeSet<u32>,
}

/// Which passes run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Passes {
    /// Remove flag writes nothing can observe ([`flags::dead_flags`]).
    pub dead_flags: bool,
    /// Turn flag writes read at most by a branch or `ReadFlag` in their own
    /// block into [`ir::Stmt::LazyFlags`] ([`lazy::lazy_flags`]): the
    /// backend computes the read flag in place and the rest on exit.
    pub lazy_flags: bool,
}

/// Turns one pass on or off.
type Setter = fn(&mut Passes, bool);

/// `(name, setter)` for every pass, in the order they run.
const NAMES: &[(&str, Setter)] =
    &[("dead-flags", |p, on| p.dead_flags = on), ("lazy-flags", |p, on| p.lazy_flags = on)];

impl Passes {
    pub const NONE: Passes = Passes { dead_flags: false, lazy_flags: false };
    pub const ALL: Passes = Passes { dead_flags: true, lazy_flags: true };

    /// Every pass name, for usage text.
    pub fn names() -> impl Iterator<Item = &'static str> {
        NAMES.iter().map(|(n, _)| *n)
    }

    /// Parse a comma-separated spec: `all`, `none`, pass names to turn on
    /// and `-name` to turn one off, applied left to right from nothing
    /// (`all,-dead-flags`; `dead-flags`).
    pub fn parse(spec: &str) -> Result<Passes, String> {
        let mut p = Passes::NONE;
        for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (on, name) = match item.strip_prefix('-') {
                Some(n) => (false, n),
                None => (true, item),
            };
            match name {
                "all" => p = if on { Passes::ALL } else { Passes::NONE },
                "none" if on => p = Passes::NONE,
                _ => match NAMES.iter().find(|(n, _)| *n == name) {
                    Some((_, set)) => set(&mut p, on),
                    None => {
                        let known: Vec<_> = Passes::names().collect();
                        return Err(format!("unknown pass {name:?} (known: all, none, {})", known.join(", ")));
                    }
                },
            }
        }
        Ok(p)
    }

    /// `H2_PASSES`, or every pass when it is unset.
    pub fn from_env() -> Result<Passes, String> {
        match std::env::var("H2_PASSES") {
            Ok(s) => Passes::parse(&s).map_err(|e| format!("H2_PASSES={s}: {e}")),
            Err(_) => Ok(Passes::ALL),
        }
    }
}

impl Default for Passes {
    fn default() -> Self {
        Passes::ALL
    }
}

/// What the passes did to one function or a whole image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Flag writes before any pass ran.
    pub flag_writes: usize,
    /// Flag writes removed as dead.
    pub dead_flags: usize,
    /// Flag writes made lazy.
    pub lazy_flags: usize,
    /// Pure statements removed because their results became unused.
    pub dead_stmts: usize,
}

impl std::ops::AddAssign for Report {
    fn add_assign(&mut self, o: Report) {
        self.flag_writes += o.flag_writes;
        self.dead_flags += o.dead_flags;
        self.lazy_flags += o.lazy_flags;
        self.dead_stmts += o.dead_stmts;
    }
}

/// Run the enabled passes over `f`.
pub fn optimize(f: &mut IrFunction, shape: &Shape<'_>, passes: Passes) -> Report {
    let mut r = Report { flag_writes: flags::count_writes(f), ..Report::default() };
    // Lazy first, so each lazy group keeps every flag its instruction
    // writes; dead-flags then removes dead eager writes and whole dead
    // groups.
    if passes.lazy_flags {
        r.lazy_flags = lazy::lazy_flags(f);
    }
    if passes.dead_flags {
        r.dead_flags = flags::dead_flags(f, shape);
    }
    if passes != Passes::NONE {
        for b in f.blocks.values_mut() {
            for i in &mut b.insns {
                r.dead_stmts += dce::instruction(i);
            }
        }
    }
    r
}
