//! A recording [`Engine`] and a coverage accumulator, for tests and for
//! running retail scripts without a game behind them.
use super::{Binding, Cell, Engine, EngineCall, EngineError, HsType, Native, Vm};
use crate::Catalog;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// What the mock answers for engine-facing calls and world queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// false, 0, none: nothing exists and nothing happens.
    Neutral,
    /// true, 1, a fresh handle: conditions pass and objects exist, so
    /// scripts advance past their `sleep_until` gates.
    Truthy,
    /// A seeded mix of the two, per call.
    Random(u32),
}

/// One recorded engine-facing call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub tick: i32,
    pub index: u16,
    pub arguments: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct RecordingEngine {
    pub time: i32,
    pub tick_rate: i16,
    pub seed: u32,
    pub policy: Policy,
    /// Builtin index -> calls.
    pub counts: BTreeMap<u16, u64>,
    /// Builtin index -> name the VM passed.
    pub names: BTreeMap<u16, String>,
    /// The first `keep` calls in order.
    pub log: Vec<Call>,
    pub keep: usize,
    pub prints: Vec<Vec<u8>>,
    pub external_reads: BTreeMap<u16, u64>,
    pub external_writes: BTreeMap<u16, u64>,
    pub object_queries: u64,
    next_handle: u32,
    mix: u32,
}

impl RecordingEngine {
    pub fn new(policy: Policy) -> Self {
        Self {
            time: 0,
            tick_rate: 30,
            seed: 0,
            policy,
            counts: BTreeMap::new(),
            names: BTreeMap::new(),
            log: Vec::new(),
            keep: 0,
            prints: Vec::new(),
            external_reads: BTreeMap::new(),
            external_writes: BTreeMap::new(),
            object_queries: 0,
            next_handle: 0,
            mix: match policy {
                Policy::Random(s) => s | 1,
                _ => 1,
            },
        }
    }
    fn truthy(&mut self) -> bool {
        match self.policy {
            Policy::Neutral => false,
            Policy::Truthy => true,
            Policy::Random(_) => {
                self.mix ^= self.mix << 13;
                self.mix ^= self.mix >> 17;
                self.mix ^= self.mix << 5;
                self.mix & 1 != 0
            }
        }
    }
    fn handle(&mut self) -> u32 {
        self.next_handle = self.next_handle.wrapping_add(1) & 0xffff;
        0xe000_0000 | self.next_handle
    }
    /// A policy value for `ty`.
    pub fn value(&mut self, ty: HsType) -> Cell {
        let yes = self.truthy();
        match ty {
            HsType::VOID => Cell::ZERO,
            HsType::BOOLEAN => Cell::from_bool(yes),
            HsType::REAL => Cell::from_real(if yes { 1.0 } else { 0.0 }),
            HsType::SHORT => Cell::from_short(yes as i16),
            HsType::LONG => Cell::from_long(yes as i32),
            HsType::STRING => Cell::EMPTY_STRING,
            HsType::STRING_ID => Cell::ZERO,
            _ if ty.size() == 2 => Cell::from_short(if yes { 0 } else { -1 }),
            _ if yes => Cell(self.handle()),
            _ => Cell::NONE,
        }
    }
}

impl Engine for RecordingEngine {
    fn game_time(&self) -> i32 {
        self.time
    }
    fn tick_rate(&self) -> i16 {
        self.tick_rate
    }
    fn random_seed(&mut self) -> &mut u32 {
        &mut self.seed
    }
    fn call(&mut self, call: EngineCall<'_>) -> Result<Cell, EngineError> {
        *self.counts.entry(call.index).or_default() += 1;
        if let Some(n) = call.name {
            self.names.entry(call.index).or_insert_with(|| n.to_owned());
        }
        if self.log.len() < self.keep {
            self.log.push(Call {
                tick: self.time,
                index: call.index,
                arguments: call.arguments.iter().map(|c| c.0).collect(),
            });
        }
        Ok(self.value(call.return_type))
    }
    fn object_type(&mut self, object: u32) -> Option<u8> {
        self.object_queries += 1;
        (object != u32::MAX && self.truthy()).then_some(0)
    }
    fn object_from_name(&mut self, name: i16) -> u32 {
        self.object_queries += 1;
        if (0..0x280).contains(&name) && self.truthy() {
            self.handle()
        } else {
            u32::MAX
        }
    }
    fn object_list_new(&mut self, _objects: &[u32]) -> u32 {
        self.handle()
    }
    fn object_list_from_ai(&mut self, _ai: u32) -> u32 {
        if self.truthy() {
            self.handle()
        } else {
            u32::MAX
        }
    }
    fn external_global_read(&mut self, index: u16, _ty: HsType) -> Option<Cell> {
        *self.external_reads.entry(index).or_default() += 1;
        None
    }
    fn external_global_write(&mut self, index: u16, _ty: HsType, _value: Cell) {
        *self.external_writes.entry(index).or_default() += 1;
    }
    fn print(&mut self, text: &[u8]) {
        if self.prints.len() < 64 {
            self.prints.push(text.to_vec());
        }
    }
}

/// How a builtin is covered, for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// The VM's own evaluator (control, threads, math, comparison).
    Native,
    /// A retail stub the VM reproduces (returns 0).
    Discarded,
    /// Has a Rust override.
    Override,
    /// Dispatched to the engine trait: not yet implemented natively.
    Engine,
    /// Null evaluator.
    Null,
}
impl Kind {
    pub fn of(b: Binding) -> Self {
        match b {
            Binding::Native(Native::DiscardedEvaluated | Native::DiscardedUnevaluated) => {
                Self::Discarded
            }
            Binding::Native(_) => Self::Native,
            Binding::Override => Self::Override,
            Binding::Engine => Self::Engine,
            Binding::Null => Self::Null,
        }
    }
}

/// Builtin coverage across many runs.
#[derive(Debug, Clone, Default)]
pub struct Coverage {
    /// Index -> (statically reachable from some root, evaluations started).
    pub builtins: BTreeMap<u16, (bool, u64)>,
    pub kinds: BTreeMap<u16, Kind>,
    pub casts: BTreeMap<(u16, u16), u64>,
    pub scripts_started: u64,
    pub scripts_total: u64,
}
impl Coverage {
    /// Merge one finished VM run, plus the scenario's static reach.
    pub fn add(&mut self, vm: &Vm<'_>, reached: &BTreeSet<u16>) {
        for &i in reached {
            self.builtins.entry(i).or_default().0 = true;
            self.kinds.insert(i, Kind::of(vm.binding(i)));
        }
        for (&i, &n) in &vm.stats().builtins {
            self.builtins.entry(i).or_default().1 += n;
            self.kinds.insert(i, Kind::of(vm.binding(i)));
        }
        for (&k, &n) in &vm.stats().casts {
            *self.casts.entry(k).or_default() += n;
        }
        self.scripts_started += vm.stats().scripts.len() as u64;
        self.scripts_total += vm.scenario().scripts().len() as u64;
    }

    /// Plain-text summary: counts per kind, then the called engine-facing
    /// builtins (still unimplemented natively) and the reachable builtins no
    /// run reached.
    pub fn report(&self, catalog: &Catalog) -> String {
        let name = |i: u16| {
            catalog
                .function(i)
                .ok()
                .and_then(|f| f.name())
                .unwrap_or("?")
                .to_owned()
        };
        let mut out = String::new();
        let mut table: BTreeMap<Kind, [usize; 3]> = BTreeMap::new();
        for (&i, &(reached, calls)) in &self.builtins {
            let row = table.entry(self.kinds[&i]).or_default();
            row[0] += reached as usize;
            row[1] += (calls > 0) as usize;
            row[2] += (reached && calls == 0) as usize;
        }
        let _ = writeln!(out, "kind        reachable  called  reachable-not-called");
        for (k, [r, c, n]) in &table {
            let _ = writeln!(out, "{:<11} {r:>9}  {c:>6}  {n:>20}", format!("{k:?}"));
        }
        let _ = writeln!(
            out,
            "scripts started {} of {}",
            self.scripts_started, self.scripts_total
        );
        let _ = writeln!(out, "casts (source -> dest: count):");
        for (&(a, b), n) in &self.casts {
            let _ = writeln!(out, "  {a:>2} -> {b:>2}: {n}");
        }
        let _ = writeln!(
            out,
            "called engine-facing builtins (unimplemented natively):"
        );
        for (&i, &(_, calls)) in &self.builtins {
            if calls > 0 && self.kinds[&i] == Kind::Engine {
                let _ = writeln!(out, "  {i:>3} {:<48} {calls}", name(i));
            }
        }
        let _ = writeln!(out, "reachable but never called:");
        for (&i, &(reached, calls)) in &self.builtins {
            if reached && calls == 0 {
                let _ = writeln!(out, "  {i:>3} {:<48} {:?}", name(i), self.kinds[&i]);
            }
        }
        out
    }
}
