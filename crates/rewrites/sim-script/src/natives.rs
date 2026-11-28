//! Rust bodies for engine builtins. A builtin with a body here runs in
//! Rust; every other engine-facing builtin still goes to the engine
//! adapter's `call` (the guest, for now). Bodies bind by the builtin's
//! name in the catalog, so one set serves every scenario.
use hs::Catalog;
use hs::vm::{Cell, Engine, EngineCall, EngineError, Override, Vm};

/// One Rust body.
pub struct Native {
    pub name: &'static str,
    pub body: Override,
}

impl Native {
    pub fn new(
        name: &'static str,
        body: impl FnMut(&mut dyn Engine, EngineCall<'_>) -> Result<Cell, EngineError> + 'static,
    ) -> Self {
        Self { name, body: Box::new(body) }
    }
}

/// The set of Rust bodies.
#[derive(Default)]
pub struct Natives {
    list: Vec<Native>,
}

impl Natives {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(mut self, native: Native) -> Self {
        self.list.push(native);
        self
    }
    pub fn len(&self) -> usize {
        self.list.len()
    }
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Bind every body whose name the catalog has, consuming the set.
    /// Returns the names the catalog lacks. VM natives (control flow,
    /// threads, arithmetic) are not overridable and keep their VM body.
    pub fn bind(self, vm: &mut Vm<'_>, catalog: &Catalog) -> Vec<&'static str> {
        let names = catalog.named_functions();
        let mut missing = Vec::new();
        for n in self.list {
            match names.iter().find(|(_, name)| **name == n.name) {
                Some((&index, _)) => vm.bindings.bind(index, n.body),
                None => missing.push(n.name),
            }
        }
        missing
    }
}
