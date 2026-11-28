//! The scripts section: the VM's persistent state, shared between the
//! world (which saves and restores it) and the runtime (which updates it).
use hs::vm::VmState;
use sim_world::{RestoreError, Store, kind};
use std::any::Any;
use std::sync::{Arc, Mutex, MutexGuard};

/// The section's layout version.
pub const VERSION: u16 = 1;

/// What the world holds for scripts. `None` before a map's scripts have
/// started: the section is then one zero byte.
#[derive(Debug, Default)]
pub struct ScriptTable {
    state: Option<VmState>,
    /// Counts installs, so a runtime knows a restore replaced its state.
    generation: u64,
}

impl ScriptTable {
    pub fn state(&self) -> Option<&VmState> {
        self.state.as_ref()
    }
    /// The runtime's commit after an update.
    pub fn set(&mut self, state: VmState) {
        self.state = Some(state);
    }
    pub fn clear(&mut self) {
        self.state = None;
        self.generation += 1;
    }
    /// Bumped by every restore and clear.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn encode(&self) -> Vec<u8> {
        match &self.state {
            None => vec![0],
            Some(s) => {
                let mut o = vec![1];
                o.extend_from_slice(&s.encode());
                o
            }
        }
    }

    fn decode(version: u16, body: &[u8]) -> Result<Option<VmState>, RestoreError> {
        let corrupt = |what| RestoreError::Corrupt { kind: kind::SCRIPTS, what };
        if version != VERSION {
            return Err(RestoreError::SectionVersion { kind: kind::SCRIPTS, version });
        }
        match body.split_first() {
            None => Err(RestoreError::Truncated),
            Some((0, [])) => Ok(None),
            Some((0, _)) => Err(RestoreError::Trailing { kind: kind::SCRIPTS }),
            Some((1, rest)) => VmState::decode(rest).map(Some).map_err(|e| match e {
                hs::vm::StateError::Truncated => RestoreError::Truncated,
                hs::vm::StateError::Trailing => RestoreError::Trailing { kind: kind::SCRIPTS },
                hs::vm::StateError::Corrupt(what) => corrupt(what),
                hs::vm::StateError::Globals { .. } => corrupt("globals"),
            }),
            Some(_) => Err(corrupt("presence byte")),
        }
    }
}

pub type Shared = Arc<Mutex<ScriptTable>>;

/// The table, whatever a panicked holder left.
pub fn lock(t: &Mutex<ScriptTable>) -> MutexGuard<'_, ScriptTable> {
    t.lock().unwrap_or_else(|e| e.into_inner())
}

/// The world's scripts store (`World::attach`).
pub struct ScriptStore(pub Shared);

impl Store for ScriptStore {
    fn kind(&self) -> u16 {
        kind::SCRIPTS
    }
    fn version(&self) -> u16 {
        VERSION
    }
    fn encode(&self) -> Vec<u8> {
        lock(&self.0).encode()
    }
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError> {
        Ok(Box::new(ScriptTable::decode(version, body)?))
    }
    fn install(&mut self, state: Box<dyn Any + Send>) {
        if let Ok(state) = state.downcast::<Option<VmState>>() {
            let mut t = lock(&self.0);
            t.state = *state;
            t.generation += 1;
        }
    }
}
