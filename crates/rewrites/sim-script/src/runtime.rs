//! The per-tick script update over the world's scripts section.
use crate::table::{Shared, lock};
use hs::vm::{Engine, Fault, Vm, VmImage, VmResult, VmState};
use hs::{Catalog, Scenario};

/// A VM bound to the world's scripts section: it reads the section before
/// an update when a restore replaced it, and writes it back after.
pub struct ScriptRuntime<'a> {
    pub vm: Vm<'a>,
    shared: Shared,
    seen: u64,
}

impl<'a> ScriptRuntime<'a> {
    /// A runtime over `shared`. A state the section already holds (a
    /// restored save) is loaded; a state for another scenario is refused.
    pub fn new(
        scenario: &'a Scenario,
        catalog: &'a Catalog,
        image: &'a VmImage,
        shared: Shared,
    ) -> Result<Self, hs::vm::StateError> {
        let mut rt = Self { vm: Vm::new(scenario, catalog, image), shared, seen: 0 };
        rt.pull()?;
        Ok(rt)
    }

    /// Load the section if a restore or clear changed it since the last sync.
    fn pull(&mut self) -> Result<(), hs::vm::StateError> {
        let t = lock(&self.shared);
        if t.generation() != self.seen {
            self.seen = t.generation();
            if let Some(state) = t.state() {
                self.vm.import_state(state.clone())?;
            }
        }
        Ok(())
    }

    fn commit(&mut self) {
        let state: VmState = self.vm.export_state();
        lock(&self.shared).set(state);
    }

    /// New-map initialization: threads and global initializers (`0x002090a0`).
    pub fn initialize(&mut self, e: &mut dyn Engine) -> VmResult<()> {
        let r = self.vm.initialize(e);
        self.commit();
        r
    }

    /// One scheduler pass (`0x00209310`).
    pub fn update(&mut self, e: &mut dyn Engine) -> Result<(), hs::vm::StateError> {
        self.pull()?;
        self.vm.update(e);
        self.commit();
        Ok(())
    }

    pub fn take_faults(&mut self) -> Vec<Fault> {
        self.vm.take_faults()
    }
}
