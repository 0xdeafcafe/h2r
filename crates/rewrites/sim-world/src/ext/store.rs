//! The extension records as one of the world's sections.
//!
//! [`ExtStore`] is the [`Store`] over the process's [`State`]
//! ([`global`]): its section is [`State::save_section`], so the live
//! records, both checkpoint slot copies and the sim hash are in
//! `World::save` and `World::sim_hash` with every other store. A restore
//! checks the whole section before it changes anything.

use std::any::Any;

use super::checkpoint::{State, global};
use crate::{RestoreError, Store, kind};

/// The section's layout version: [`State`]'s section format.
pub const VERSION: u16 = 1;

/// The process's extension state as the world's `kind::EXT` store.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExtStore;

impl Store for ExtStore {
    fn kind(&self) -> u16 {
        kind::EXT
    }
    fn version(&self) -> u16 {
        VERSION
    }
    fn encode(&self) -> Vec<u8> {
        global().save_section()
    }
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError> {
        if version != VERSION {
            return Err(RestoreError::Corrupt { kind: kind::EXT, what: "extension section version" });
        }
        let mut state = State::new();
        state
            .load_section(body)
            .map_err(|_| RestoreError::Corrupt { kind: kind::EXT, what: "extension section does not decode" })?;
        Ok(Box::new(state))
    }
    fn install(&mut self, state: Box<dyn Any + Send>) {
        if let Ok(state) = state.downcast::<State>() {
            *global() = *state;
        }
    }
}
