//! The function-binding layer between the VM and the rest of the game.
//!
//! Builtins split three ways ([`Binding`]):
//!
//! - **native**: control flow, threads, math, comparison, and the
//!   retail-discarded stubs, implemented by the VM from the evaluator
//!   bodies (see [`super::Native`]);
//! - **override**: an engine-facing builtin given a Rust body with
//!   [`Bindings::bind`], one function at a time;
//! - **engine**: everything else goes to [`Engine::call`] with its evaluated
//!   arguments. An implementation can be a Rust subsystem, a mock, or an
//!   adapter that calls the recompiled game's own evaluator at
//!   [`EngineCall::evaluate`].
//!
//! The engine also answers the few world queries the native evaluators and
//! casts make (game time, object types, object lists, external globals).
use super::value::{Cell, HsType};
use std::collections::BTreeMap;
use std::fmt;

/// Fixed-signature builtin call, after argument evaluation (`0x00209d50`).
#[derive(Debug, Clone, Copy)]
pub struct EngineCall<'a> {
    pub index: u16,
    /// Learned from map tokens; `None` for builtins no map names.
    pub name: Option<&'a str>,
    /// The descriptor's guest evaluate address.
    pub evaluate: u32,
    pub return_type: HsType,
    pub parameters: &'a [u16],
    pub arguments: &'a [Cell],
    /// The calling thread's slot.
    pub thread: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// No binding exists for this builtin.
    Unimplemented,
    Failed(String),
}
impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unimplemented => f.write_str("unimplemented"),
            Self::Failed(m) => f.write_str(m),
        }
    }
}

/// Scenario-local globals available to native script extensions. The
/// external globals and the VM's object-list bookkeeping stay in the VM.
pub struct ScenarioGlobals<'a> {
    pub(super) cells: &'a mut [Cell],
    pub(super) definitions: &'a [crate::Global],
}

impl ScenarioGlobals<'_> {
    pub fn get(&self, index: u32, ty: HsType) -> Option<Cell> {
        let index = index as usize;
        let definition = self.definitions.get(index)?;
        (definition.value_type() == ty.0).then(|| self.cells[index])
    }

    pub fn set(&mut self, index: u32, ty: HsType, value: Cell) -> bool {
        if ty == HsType::OBJECT_LIST || self.get(index, ty).is_none() {
            return false;
        }
        let cell = &mut self.cells[index as usize];
        *cell = cell.store(ty, value);
        true
    }
}

/// Everything the VM needs from the game.
pub trait Engine {
    /// Game-time tick count, `game_time + 8` (`0x00510c54`).
    fn game_time(&self) -> i32;
    /// Ticks per second, `game_time + 2`; `sleep` scales 30-Hz authored
    /// ticks by `rate / 30`.
    fn tick_rate(&self) -> i16 {
        30
    }
    /// The game-globals byte at `+0x1120` that `0x00209850` tests before it
    /// lets a future wake time stop a thread. Its meaning is not yet
    /// recovered; when false, sleeping threads run on within the tick.
    fn sleeps_yield(&self) -> bool {
        true
    }
    /// The shared engine random seed (`*0x004e7408`); `begin_random`,
    /// `random_range` and `real_random_range` step it as an LCG.
    fn random_seed(&mut self) -> &mut u32;

    /// An engine-facing builtin.
    fn call(&mut self, call: EngineCall<'_>) -> Result<Cell, EngineError>;

    /// An engine builtin that can inspect or update scenario globals. Retail
    /// extensions can access these values by index in the guest interpreter.
    fn call_with_globals(
        &mut self,
        call: EngineCall<'_>,
        _globals: ScenarioGlobals<'_>,
    ) -> Result<Cell, EngineError> {
        self.call(call)
    }

    /// Object type byte (object `+0xaa`) of a live object, for casts.
    fn object_type(&mut self, object: u32) -> Option<u8>;
    /// Object-name index to object handle (`0x000bb760`): the table at
    /// `0x004de2d0`, -1 outside `0..0x280` or when not placed.
    fn object_from_name(&mut self, name: i16) -> u32;
    /// A new object list holding `objects` (`0x001ded60`/`0x001dedb0`).
    fn object_list_new(&mut self, objects: &[u32]) -> u32;
    /// Object list of an AI reference's actors (`0x00272ea0`).
    fn object_list_from_ai(&mut self, ai: u32) -> u32;
    /// Object-list reference count change for an object-list global
    /// (`set`, `0x00208070`; global initialization, `0x002091f6`).
    fn object_list_reference(&mut self, _list: u32, _delta: i16) {}
    /// After a global store (`0x0020a490`): the retail engine marks objects
    /// held by object/object-list globals (flag `0x10000000`).
    fn global_stored(&mut self, _ty: HsType, _value: Cell) {}

    /// Refresh an external global from engine state (`0x0020a2e0`).
    /// `None` reads as the null-pointer default ([`HsType::unbound_external`]).
    fn external_global_read(&mut self, _index: u16, _ty: HsType) -> Option<Cell> {
        None
    }
    /// Write an external global back (`0x0020a490`).
    fn external_global_write(&mut self, _index: u16, _ty: HsType, _value: Cell) {}

    /// `print` is a stub in retail; the VM reports its text here.
    fn print(&mut self, _text: &[u8]) {}
    /// Console `inspect` output, only on console-evaluated threads.
    fn inspect(&mut self, _ty: HsType, _value: Cell) {}
    /// End of `hs_update` (tail jump to `0x001deed0`).
    fn after_update(&mut self) {}
}

/// Rust body for one engine-facing builtin.
pub type Override = Box<dyn FnMut(&mut dyn Engine, EngineCall<'_>) -> Result<Cell, EngineError>>;

/// How one builtin index is dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Native(super::Native),
    Override,
    Engine,
    /// Null evaluate pointer (`cond`, index 3): the retail engine would call
    /// address 0.
    Null,
}

/// Per-index Rust overrides for engine-facing builtins.
#[derive(Default)]
pub struct Bindings {
    overrides: BTreeMap<u16, Override>,
}
impl fmt::Debug for Bindings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.overrides.keys()).finish()
    }
}
impl Bindings {
    /// Give one engine-facing builtin a Rust body. Natives cannot be
    /// overridden; the VM ignores an override on a native index.
    pub fn bind(&mut self, index: u16, body: Override) {
        self.overrides.insert(index, body);
    }
    pub fn is_bound(&self, index: u16) -> bool {
        self.overrides.contains_key(&index)
    }
    pub(crate) fn get(&mut self, index: u16) -> Option<&mut Override> {
        self.overrides.get_mut(&index)
    }
}
