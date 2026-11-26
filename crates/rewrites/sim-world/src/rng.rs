//! The shared engine random stream (`[0x4e7408]`): retail's LCG
//! `seed * 0x19660d + 0x3c6ef35f` mod 2^32, one stream for the whole
//! simulation. Callers draw in slot order, so the stream a tick consumes is
//! the same on every run.

use crate::save::{Reader, RestoreError, Writer};

const MUL: u32 = 0x0019_660d;
const ADD: u32 = 0x3c6e_f35f;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng(u32);

impl Rng {
    /// Section version of the saved state.
    pub const VERSION: u16 = 1;

    pub fn new(seed: u32) -> Self {
        Self(seed)
    }

    pub fn state(&self) -> u32 {
        self.0
    }

    /// Step the seed and return the new seed, as retail's draw does.
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(MUL).wrapping_add(ADD);
        self.0
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(self.0);
        w.finish()
    }

    pub(crate) fn decode(version: u16, body: &[u8]) -> Result<Self, RestoreError> {
        if version != Self::VERSION {
            return Err(RestoreError::SectionVersion {
                kind: crate::kind::RNG,
                version,
            });
        }
        let mut r = Reader::new(body);
        let seed = r.u32()?;
        r.end(crate::kind::RNG)?;
        Ok(Self(seed))
    }
}
