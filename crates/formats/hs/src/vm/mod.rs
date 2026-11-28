//! A native HaloScript runtime with the retail evaluator's semantics.
//!
//! The VM executes a decoded [`Scenario`] against a [`Catalog`] and an
//! [`Engine`]. It mirrors the guest interpreter's structure rather than
//! re-deriving Lisp semantics: thread datums with frame stacks, evaluators
//! that are re-entered once per loop iteration and keep their progress in
//! frame locals, value slots converted through the retail cast table, and
//! the scheduler's slot-order tick. Evidence and addresses are in
//! `docs/engine/hs_vm.md`.
//!
//! Host policies that the retail engine does not have (and that therefore
//! report a [`Fault`] instead of corrupting memory): the 1,024-byte frame
//! stack is bounded, one thread execution is bounded in steps, and a null
//! cast or evaluator is an error rather than a call to address 0.
mod convert;
mod engine;
mod eval;
mod image;
pub mod mock;
mod state;
mod thread;
mod value;

pub use convert::{Cast, PureCast, apply as apply_pure_cast, classify, cvttss2si, fistp};
pub use engine::{Binding, Bindings, Engine, EngineCall, EngineError, Override, ScenarioGlobals};
pub use eval::{ArgumentStep, argument_step};
pub use image::{Native, SLEEP_UNTIL_INDEX, VmImage};
pub use thread::{Category, DORMANT, FINISHED, Schedule, ScheduleSlot, Thread, ThreadState};
pub use state::{StateError, VmState};
pub use value::{Cell, HsType, StringRef, Value};

use crate::{Catalog, Datum, Node, Scenario};
use std::collections::BTreeMap;
use std::fmt;
use thread::{Dest, FIRST_ENTRY, Pool};

/// External globals occupy the first `0x41d` global cells (`0x00208ff0`).
pub const EXTERNAL_GLOBALS: usize = 0x41d;
/// Inline frame stack per thread.
pub const STACK_BYTES: usize = 0x400;
/// Host bound on evaluator dispatches in one thread execution. Retail
/// evaluation cannot loop without sleeping, so hitting this is a fault.
pub const STEP_LIMIT: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
pub enum VmError {
    /// Malformed or stale node reached at run time.
    Node(String),
    /// Null conversion-table entry (`0x004f5770[dest*62+source]`).
    InvalidCast {
        from: HsType,
        to: HsType,
    },
    /// Null evaluate pointer (`cond`).
    NullEvaluator {
        index: u16,
    },
    /// Custom-argument builtin the VM does not implement.
    UnsupportedCustom {
        index: u16,
    },
    /// `0x00209d50` found an argument of the wrong type or a missing one.
    Argument {
        index: u16,
        position: usize,
    },
    /// A node of string type that is not an atom: the load-time string fixup
    /// (`0x002a0a28`) would have overwritten its data word.
    StringFixup {
        datum: Datum,
    },
    /// `sleep_forever`'s script argument is not an atom.
    NonAtomScript,
    /// `begin_random` with more than 32 branches overruns its one-word bitmap.
    BeginRandomOverflow {
        branches: usize,
    },
    /// Object cast index outside the mask table.
    CastMask {
        index: u16,
    },
    StackOverflow,
    StepLimit,
    ThreadPoolFull,
    GlobalIndex(u16),
    /// A global initializer slept; the guest has no defined behavior here.
    InitializerYielded,
    Engine {
        index: u16,
        error: EngineError,
    },
}
impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(m) => write!(f, "bad node: {m}"),
            Self::InvalidCast { from, to } => write!(f, "no cast from type {} to {}", from.0, to.0),
            Self::NullEvaluator { index } => write!(f, "builtin {index} has a null evaluator"),
            Self::UnsupportedCustom { index } => write!(f, "custom builtin {index} is not native"),
            Self::Argument { index, position } => {
                write!(
                    f,
                    "builtin {index}: argument {position} missing or mistyped"
                )
            }
            Self::StringFixup { datum } => {
                write!(
                    f,
                    "string-typed non-atom {:#010x} is clobbered by the load fixup",
                    datum.0
                )
            }
            Self::NonAtomScript => f.write_str("sleep_forever script argument is not an atom"),
            Self::BeginRandomOverflow { branches } => {
                write!(
                    f,
                    "begin_random with {branches} branches overruns its bitmap"
                )
            }
            Self::CastMask { index } => write!(f, "object cast {index} has no type mask"),
            Self::StackOverflow => f.write_str("thread stack exceeds 1024 bytes"),
            Self::StepLimit => f.write_str("thread exceeded the step limit"),
            Self::ThreadPoolFull => f.write_str("thread pool is full"),
            Self::GlobalIndex(i) => write!(f, "global index {i:#06x} out of range"),
            Self::InitializerYielded => f.write_str("a global initializer slept"),
            Self::Engine { index, error } => write!(f, "builtin {index}: {error}"),
        }
    }
}
impl std::error::Error for VmError {}
pub type VmResult<T> = std::result::Result<T, VmError>;

impl From<crate::Error> for VmError {
    fn from(e: crate::Error) -> Self {
        Self::Node(e.to_string())
    }
}

/// A thread stopped by an error. The thread is finished (or deleted, for
/// transient threads); other threads keep running.
#[derive(Debug, Clone, PartialEq)]
pub struct Fault {
    pub thread: usize,
    pub script: Option<u16>,
    pub expression: Datum,
    pub error: VmError,
}

/// Execution counters, for coverage reports.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// Builtin index -> evaluations started.
    pub builtins: BTreeMap<u16, u64>,
    /// Script index -> calls (script-call nodes and thread starts).
    pub scripts: BTreeMap<u16, u64>,
    /// (source, dest) -> non-identity casts performed.
    pub casts: BTreeMap<(u16, u16), u64>,
    pub dispatches: u64,
    pub thread_executions: u64,
    pub updates: u64,
}

pub struct Vm<'a> {
    scenario: &'a Scenario,
    catalog: &'a Catalog,
    image: &'a VmImage,
    pub bindings: Bindings,
    pool: Pool,
    globals: Vec<Cell>,
    enabled: bool,
    current: Option<usize>,
    host_strings: Vec<Vec<u8>>,
    faults: Vec<Fault>,
    stats: Stats,
    /// Set by a completing transient thread, read by `evaluate`.
    last_result: Option<Cell>,
    /// Frame index of the evaluator being dispatched.
    frame: usize,
}

impl<'a> Vm<'a> {
    /// `hs_runtime_initialize` (`0x00208ff0`): empty thread pool, zeroed
    /// external and scenario global cells, scheduler disabled.
    pub fn new(scenario: &'a Scenario, catalog: &'a Catalog, image: &'a VmImage) -> Self {
        Self {
            scenario,
            catalog,
            image,
            bindings: Bindings::default(),
            pool: Pool::new(),
            globals: vec![Cell::ZERO; EXTERNAL_GLOBALS + scenario.globals.len()],
            enabled: false,
            current: None,
            host_strings: vec![Vec::new()],
            faults: Vec::new(),
            stats: Stats::default(),
            last_result: None,
            frame: 0,
        }
    }

    /// The state that persists between updates: threads with their frames,
    /// the thread pool's allocation cursor and salt, global cells, the
    /// scheduler switch and host strings. Counters, faults and the
    /// in-update cursors are not state.
    pub fn export_state(&self) -> VmState {
        VmState {
            pool: self.pool.clone(),
            globals: self.globals.clone(),
            enabled: self.enabled,
            host_strings: self.host_strings.clone(),
        }
    }

    /// Put an exported state back. The globals must be this scenario's
    /// count; a state from another scenario is refused.
    pub fn import_state(&mut self, state: VmState) -> Result<(), StateError> {
        if state.globals.len() != self.globals.len() {
            return Err(StateError::Globals { found: state.globals.len(), expected: self.globals.len() });
        }
        self.pool = state.pool;
        self.globals = state.globals;
        self.enabled = state.enabled;
        self.host_strings = state.host_strings;
        self.current = None;
        self.last_result = None;
        self.frame = 0;
        Ok(())
    }

    pub fn scenario(&self) -> &Scenario {
        self.scenario
    }
    pub fn stats(&self) -> &Stats {
        &self.stats
    }
    pub fn faults(&self) -> &[Fault] {
        &self.faults
    }
    pub fn take_faults(&mut self) -> Vec<Fault> {
        std::mem::take(&mut self.faults)
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    /// Occupied thread slots in scheduler order.
    pub fn threads(&self) -> impl Iterator<Item = (usize, &Thread)> {
        self.pool
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.as_ref().map(|t| (i, t)))
    }
    pub fn thread(&self, slot: usize) -> Option<&Thread> {
        self.pool.slots.get(slot)?.as_ref()
    }
    /// First thread running `script` (`0x00209e70`).
    pub fn script_thread(&self, script: u16) -> Option<usize> {
        self.threads()
            .find(|(_, t)| t.script == Some(script))
            .map(|(i, _)| i)
    }
    /// Scenario global cell by definition index.
    pub fn global(&self, index: usize) -> Option<Cell> {
        self.globals.get(EXTERNAL_GLOBALS + index).copied()
    }
    /// Give a scenario global's cell a value read from elsewhere (a
    /// running game's cell). `false` for an index past the definitions.
    pub fn set_global(&mut self, index: usize, cell: Cell) -> bool {
        match self.globals.get_mut(EXTERNAL_GLOBALS + index) {
            Some(slot) if index < self.scenario.globals.len() => {
                *slot = cell;
                true
            }
            _ => false,
        }
    }
    pub fn global_by_name(&self, name: &str) -> Option<Value> {
        let (i, g) = self
            .scenario
            .globals
            .iter()
            .enumerate()
            .find(|(_, g)| g.name().ok() == Some(name))?;
        Some(Value::decode(HsType(g.value_type), self.global(i)?))
    }
    pub fn external_global(&self, index: usize) -> Option<Cell> {
        (index < EXTERNAL_GLOBALS).then(|| self.globals[index])
    }
    /// Text of a string slot.
    pub fn string(&self, c: Cell) -> Option<&[u8]> {
        match StringRef::from_cell(c) {
            StringRef::Script(o) => self.scenario.string_bytes(o).ok(),
            StringRef::Host(i) => self.host_strings.get(i as usize).map(Vec::as_slice),
        }
    }
    /// Intern engine-supplied text as a string slot value.
    pub fn host_string(&mut self, text: &[u8]) -> Cell {
        if let Some(i) = self.host_strings.iter().position(|s| s == text) {
            return StringRef::Host(i as u32).cell();
        }
        self.host_strings.push(text.to_vec());
        StringRef::Host(self.host_strings.len() as u32 - 1).cell()
    }

    /// How a builtin index is dispatched.
    pub fn binding(&self, index: u16) -> Binding {
        let Ok(f) = self.catalog.function(index) else {
            return Binding::Null;
        };
        if f.evaluate == 0 {
            return Binding::Null;
        }
        if let Some(n) = Native::at(f.evaluate) {
            return Binding::Native(n);
        }
        if let Some(&n) = self.image.discarded.get(&index) {
            return Binding::Native(n);
        }
        if self.bindings.is_bound(index) {
            Binding::Override
        } else {
            Binding::Engine
        }
    }

    /// New-map initialization (`0x002090a0`): delete all threads, enable the
    /// scheduler, evaluate every scenario global initializer in table order
    /// on a category-1 thread (running it to completion when it is a call),
    /// then create one thread per startup, dormant and continuous script in
    /// script-table order. Static and stub scripts get no thread.
    pub fn initialize(&mut self, e: &mut dyn Engine) -> VmResult<()> {
        self.pool.clear();
        self.enabled = true;
        self.current = None;
        let t = self
            .pool
            .allocate(Category::GlobalInit, None, false)
            .ok_or(VmError::ThreadPoolFull)?;
        for i in 0..self.scenario.globals.len() {
            if let Err(error) = self.initialize_global(e, t, i) {
                let th = self.th(t);
                let expression = th.frames.last().map_or(Datum::NULL, |f| f.expr);
                th.frames.truncate(1);
                self.current = None;
                self.faults.push(Fault {
                    thread: t,
                    script: None,
                    expression,
                    error,
                });
            }
        }
        self.pool.delete(t);
        for i in 0..self.scenario.scripts.len() {
            if self.scenario.scripts[i].kind <= 2 {
                self.create_script_thread(i as u16)?;
            }
        }
        Ok(())
    }

    /// Create an ordinary script thread at its root frame. Map initialization
    /// does this for every scheduled definition; a VM checking one guest
    /// thread can load its globals and create only that definition.
    pub fn create_script_thread(&mut self, script: u16) -> VmResult<usize> {
        let definition = self.scenario.scripts.get(usize::from(script))
            .filter(|s| s.kind <= 2)
            .ok_or_else(|| VmError::Node("expected an ordinary script definition".into()))?;
        self.pool.allocate(Category::Script, Some(script), definition.kind == 1)
            .ok_or(VmError::ThreadPoolFull)
    }

    fn initialize_global(&mut self, e: &mut dyn Engine, t: usize, i: usize) -> VmResult<()> {
        let g = &self.scenario.globals[i];
        let (root, ty) = (g.root, HsType(g.value_type));
        let slot = EXTERNAL_GLOBALS + i;
        self.globals[slot] = Cell::ZERO;
        {
            let th = self.th(t);
            th.script = None;
            th.frames.truncate(1);
            th.frames[0].locals.clear();
        }
        self.eval(e, t, root, Dest::Global(slot))?;
        if self.th(t).flags & FIRST_ENTRY != 0 {
            self.execute(e, t)?;
            if self.th(t).frames.len() > 1 {
                return Err(VmError::InitializerYielded);
            }
            if ty == HsType::OBJECT_LIST && self.globals[slot] != Cell::NONE {
                e.object_list_reference(self.globals[slot].0, 1);
            }
        }
        self.write_back(e, i as u16)
    }

    /// `hs_update` (`0x00209310`): read the tick once, then walk occupied
    /// thread slots in order, running every thread that is not category 4,
    /// not negative-wake and due (wake <= tick). Stops early if the scheduler
    /// is disabled. Faulting threads are stopped and reported.
    pub fn update(&mut self, e: &mut dyn Engine) {
        if !self.enabled {
            return;
        }
        self.stats.updates += 1;
        let mut schedule = Schedule::new(e.game_time());
        while self.enabled {
            let Ok(next) = schedule.next_ready(|start| {
                Ok::<_, std::convert::Infallible>(self.pool.next(start).map(|i| {
                    let thread = self.pool.slots[i].as_ref().unwrap();
                    ScheduleSlot {
                        index: i,
                        external: thread.category == Category::External,
                        wake: thread.wake,
                        thread: i,
                    }
                }))
            });
            let Some(i) = next else { break };
            self.run(e, i);
        }
        e.after_update();
    }

    /// Evaluate an expression on a new transient thread, like the console
    /// path (`0x002093d0`): an atom completes at once; a call runs until it
    /// completes or sleeps, and a sleeping thread continues under
    /// [`update`](Self::update) until it completes, then is deleted.
    /// Returns the value if it completed, else the thread slot.
    pub fn evaluate(
        &mut self,
        e: &mut dyn Engine,
        expr: Datum,
        inspect: bool,
    ) -> VmResult<Result<Cell, usize>> {
        if !self.enabled {
            return Ok(Ok(Cell::ZERO));
        }
        let t = self
            .pool
            .allocate(Category::Transient, None, false)
            .ok_or(VmError::ThreadPoolFull)?;
        if inspect {
            self.th(t).flags |= thread::INSPECT;
        }
        self.th(t).frames[0].bytes = 4;
        if let Err(error) = self.eval(e, t, expr, Dest::Local(0)) {
            self.pool.delete(t);
            return Err(error);
        }
        if self.th(t).flags & FIRST_ENTRY == 0 {
            let v = self.th(t).frames[0].local(0);
            self.pool.delete(t);
            return Ok(Ok(v));
        }
        let faults = self.faults.len();
        self.last_result = None;
        self.run(e, t);
        if let Some(f) = self.faults.get(faults) {
            return Err(f.error.clone());
        }
        Ok(match self.last_result.take() {
            Some(v) => Ok(v),
            None => Err(t),
        })
    }

    /// Run a script's body on a transient thread, as a console call of it
    /// would. Used to exercise static scripts.
    pub fn call_script(
        &mut self,
        e: &mut dyn Engine,
        script: u16,
    ) -> VmResult<Result<Cell, usize>> {
        let s = self
            .scenario
            .scripts
            .get(script as usize)
            .ok_or_else(|| VmError::Node("script index out of range".into()))?;
        *self.stats.scripts.entry(script).or_default() += 1;
        self.evaluate(e, s.root, false)
    }

    /// Host control: delete a thread outright (no retail evaluator does this
    /// to a running thread; validation uses it to bound abandoned waits).
    pub fn stop_thread(&mut self, slot: usize) {
        if self.thread(slot).is_some() {
            self.pool.delete(slot);
        }
    }

    /// Wake a script's thread, as `wake` does (`0x00209c80`).
    pub fn wake_script(&mut self, script: u16) -> VmResult<bool> {
        match self.script_thread(script) {
            Some(t) => {
                self.wake_thread(t)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn th(&mut self, t: usize) -> &mut Thread {
        self.pool.slots[t].as_mut().expect("live thread")
    }
    fn node(&self, d: Datum) -> VmResult<Node> {
        Ok(*self.scenario.node(d)?)
    }

    /// Execute with fault containment.
    fn run(&mut self, e: &mut dyn Engine, t: usize) {
        if let Err(error) = self.execute(e, t) {
            let th = self.th(t);
            let expression = th.frames.last().map_or(Datum::NULL, |f| f.expr);
            let fault = Fault {
                thread: t,
                script: th.script,
                expression,
                error,
            };
            if th.category == Category::Transient {
                self.pool.delete(t);
            } else {
                th.frames.truncate(1);
                th.wake = FINISHED;
            }
            self.current = None;
            self.faults.push(fault);
        }
    }

    /// `hs_thread_execute` (`0x00209850`).
    fn execute(&mut self, e: &mut dyn Engine, t: usize) -> VmResult<()> {
        self.current = Some(t);
        self.stats.thread_executions += 1;
        let (category, script) = {
            let th = self.th(t);
            th.wake = 0;
            (th.category, th.script)
        };
        let definition = match category {
            Category::Script | Category::External => {
                let s =
                    script.ok_or_else(|| VmError::Node("script thread without script".into()))?;
                Some(
                    self.scenario
                        .scripts
                        .get(s as usize)
                        .ok_or_else(|| VmError::Node("script index out of range".into()))?,
                )
            }
            _ => None,
        };
        if self.th(t).frames.len() == 1 {
            let d = definition.ok_or_else(|| VmError::Node("thread has no expression".into()))?;
            *self.stats.scripts.entry(script.unwrap()).or_default() += 1;
            let root = &mut self.th(t).frames[0];
            root.locals.clear();
            root.bytes = 4;
            self.eval(e, t, d.root, Dest::Local(0))?;
        }
        let mut steps = 0;
        while self.th(t).frames.len() > 1 {
            let wake = self.th(t).wake;
            if wake < 0 || (e.sleeps_yield() && e.game_time() < wake) || !self.enabled {
                break;
            }
            steps += 1;
            if steps > STEP_LIMIT {
                return Err(VmError::StepLimit);
            }
            let th = self.th(t);
            let first = th.flags & FIRST_ENTRY != 0;
            th.flags &= !FIRST_ENTRY;
            self.step(e, t, first)?;
            if self.pool.slots.get(t).is_none_or(Option::is_none) {
                break;
            }
            if self.th(t).stack_bytes() > STACK_BYTES {
                return Err(VmError::StackOverflow);
            }
        }
        if self.th(t).frames.len() == 1 {
            match category {
                Category::Script => {
                    if definition.is_some_and(|d| d.kind <= 1) {
                        self.th(t).wake = FINISHED;
                    }
                }
                Category::External => self.th(t).wake = FINISHED,
                Category::Transient => {
                    self.last_result = Some(self.th(t).frames[0].local(0));
                    self.pool.delete(t);
                }
                Category::GlobalInit => {}
            }
        }
        self.current = None;
        Ok(())
    }

    /// `0x002099f0`: atoms are read (and cast) into `dest` at once; calls
    /// push a frame, record `dest` in the current frame and set first-entry.
    fn eval(&mut self, e: &mut dyn Engine, t: usize, expr: Datum, dest: Dest) -> VmResult<()> {
        let n = self.node(expr)?;
        if !n.atom() {
            if n.value_type == HsType::STRING.0 {
                return Err(VmError::StringFixup { datum: expr });
            }
            let th = self.th(t);
            th.top().pending = dest;
            th.frames.push(thread::Frame::new(expr));
            th.flags |= FIRST_ENTRY;
            return Ok(());
        }
        let value = self.atom(e, expr, &n)?;
        self.write(t, dest, value);
        Ok(())
    }

    /// The atom half of `0x002099f0`: a global read (external refreshed
    /// first) cast from the global's declared type, or a literal cast from
    /// its source type.
    pub(crate) fn atom(&mut self, e: &mut dyn Engine, expr: Datum, n: &Node) -> VmResult<Cell> {
        if n.global() {
            if n.value_type == HsType::STRING.0 {
                return Err(VmError::StringFixup { datum: expr });
            }
            let word = crate::expression::global_word(n.data)?;
            let ty = self.global_type(word)?;
            let v = self.read_global(e, word)?;
            self.cast(e, ty, HsType(n.value_type), v)
        } else {
            let raw = if n.value_type == HsType::STRING.0 {
                // The load-time fixup replaces the data word of every
                // string-typed node with its string's address.
                StringRef::Script(n.string_offset).cell()
            } else {
                Cell(n.data)
            };
            self.cast(e, HsType(n.opcode), HsType(n.value_type), raw)
        }
    }

    fn write(&mut self, t: usize, dest: Dest, value: Cell) {
        match dest {
            Dest::Local(i) => self.th(t).top().set_local(i, value),
            Dest::Global(g) => self.globals[g] = value,
        }
    }

    /// `0x00209ae0`: cast the result from the builtin's (or script's)
    /// declared return type to the node's type, write it through the parent's
    /// pending destination and pop the frame.
    fn ret(&mut self, e: &mut dyn Engine, t: usize, value: Cell) -> VmResult<()> {
        let expr = self.th(t).top().expr;
        let n = self.node(expr)?;
        let source = if n.script_call() {
            self.scenario
                .scripts
                .get(n.opcode as usize)
                .ok_or_else(|| VmError::Node("script index out of range".into()))?
                .return_type
        } else {
            self.catalog.function(n.opcode)?.return_type
        };
        let v = self.cast(e, HsType(source), HsType(n.value_type), value)?;
        let th = self.th(t);
        th.frames.pop();
        let dest = th.top().pending;
        self.write(t, dest, v);
        Ok(())
    }

    /// Apply the cast rules (`0x0020a290` / `0x00209ae0`).
    fn cast(&mut self, e: &mut dyn Engine, from: HsType, to: HsType, c: Cell) -> VmResult<Cell> {
        let cast = classify(from, to);
        if cast != Cast::Identity {
            *self.stats.casts.entry((from.0, to.0)).or_default() += 1;
        }
        Ok(match cast {
            Cast::Identity => c,
            Cast::Pure(p) => {
                let text_len = || self.string(c).map_or(0, <[u8]>::len);
                convert::apply(p, c, text_len)
            }
            Cast::ObjectFromName => Cell(e.object_from_name(c.short())),
            Cast::ObjectListFromObject => {
                if c == Cell::NONE {
                    Cell::NONE
                } else {
                    Cell(e.object_list_new(&[c.0]))
                }
            }
            Cast::ObjectListFromName => {
                let o = e.object_from_name(c.short());
                if o == u32::MAX {
                    Cell::NONE
                } else {
                    Cell(e.object_list_new(&[o]))
                }
            }
            Cast::ObjectListFromAi => Cell(e.object_list_from_ai(c.0)),
            Cast::Invalid => return Err(VmError::InvalidCast { from, to }),
        })
    }

    fn global_slot(&self, word: u16) -> VmResult<usize> {
        let i = (word & 0x7fff) as usize;
        let slot = if word & 0x8000 != 0 {
            i
        } else {
            EXTERNAL_GLOBALS + i
        };
        if (word & 0x8000 != 0 && i >= EXTERNAL_GLOBALS) || slot >= self.globals.len() {
            return Err(VmError::GlobalIndex(word));
        }
        Ok(slot)
    }
    fn global_type(&self, word: u16) -> VmResult<HsType> {
        let i = (word & 0x7fff) as usize;
        let t = if word & 0x8000 != 0 {
            self.catalog.external_global_types.get(i).copied()
        } else {
            self.scenario.globals.get(i).map(|g| g.value_type)
        };
        t.map(HsType).ok_or(VmError::GlobalIndex(word))
    }
    /// `0x00209bc0`: refresh an external global from the engine, then read.
    fn read_global(&mut self, e: &mut dyn Engine, word: u16) -> VmResult<Cell> {
        let slot = self.global_slot(word)?;
        if word & 0x8000 != 0 {
            let ty = self.global_type(word)?;
            if ty.external_storage_width() != 0 {
                let v = e
                    .external_global_read(word & 0x7fff, ty)
                    .unwrap_or(Cell(ty.unbound_external()));
                self.globals[slot] = self.globals[slot].store(ty, v);
            }
        }
        Ok(self.globals[slot])
    }
    /// `0x0020a490`: write an external back, then let the engine mark the
    /// objects an object or object-list global holds.
    fn write_back(&mut self, e: &mut dyn Engine, word: u16) -> VmResult<()> {
        let slot = self.global_slot(word)?;
        let ty = self.global_type(word)?;
        let v = self.globals[slot];
        if word & 0x8000 != 0 {
            e.external_global_write(word & 0x7fff, ty, v);
        }
        if (ty.is_object() || ty == HsType::OBJECT_LIST) && v != Cell::NONE {
            e.global_stored(ty, v);
        }
        Ok(())
    }

    /// `0x00209c80`.
    fn wake_thread(&mut self, t: usize) -> VmResult<()> {
        let sleep_until = |vm: &Self, d: Datum| -> VmResult<bool> {
            Ok(d != Datum::NULL && vm.node(d)?.opcode == SLEEP_UNTIL_INDEX)
        };
        let (wake, flags, top, parent) = {
            let th = self.th(t);
            let n = th.frames.len();
            let top = th.frames[n - 1].expr;
            let parent = (n >= 2).then(|| th.frames[n - 2].expr);
            (th.wake, th.flags, top, parent)
        };
        if wake == FINISHED {
            return Ok(());
        }
        self.th(t).wake = 0;
        if flags & thread::SAVED_SLEEP != 0 {
            let th = self.th(t);
            th.wake = th.saved_wake;
            th.flags &= !thread::SAVED_SLEEP;
            return Ok(());
        }
        if sleep_until(self, top)? {
            self.th(t).frames.pop();
        } else if let Some(p) = parent
            && sleep_until(self, p)?
        {
            let th = self.th(t);
            th.frames.truncate(th.frames.len() - 2);
            th.flags &= !FIRST_ENTRY;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
