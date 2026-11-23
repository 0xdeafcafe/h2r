//! The `Digest` record: what the host says its set is, and the client's
//! check of its own replica against it. See docs/game/ai-networking.md,
//! "Determinism and digests".

use crate::set::ActorSet;
use crate::wire::Record;

/// Disagreeing digests in a row after which the client closes its channel
/// for the match. Each after the first counts only once a key has been
/// applied since the one before, so a slow or lost key is asked for again
/// and never closes the channel.
pub const DIVERGED_AFTER: u8 = 3;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The mode state's hash in a digest: 64-bit FNV-1a over the mode bytes.
/// The empty state hashes to the FNV offset basis.
pub fn state_hash(state: &[u8]) -> u64 {
    state.iter().fold(FNV_OFFSET, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

impl ActorSet {
    /// The host's digest of this set, with the host log head it has
    /// written (all zeros when no mode keeps a log).
    pub fn digest(&self, log_head: [u8; 32]) -> Record {
        Record::Digest {
            log_head,
            state_hash: state_hash(self.state()),
            actor_count: self.actors().len() as u16,
        }
    }

    /// Whether this set has the digest's mode state and actor count.
    pub fn agrees_with(&self, hash: u64, actor_count: u16) -> bool {
        state_hash(self.state()) == hash && self.actors().len() == usize::from(actor_count)
    }
}

/// What a client does after one digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The replica agrees with the host.
    Agrees,
    /// The replica differs: ask the host for a key.
    RequestKey,
    /// [`DIVERGED_AFTER`] digests in a row differ with keys applied
    /// between them: keys do not repair the replica, so the channel
    /// closes for the match.
    Diverged,
}

/// One client's run of disagreeing digests. Start a fresh one with each
/// match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Check {
    misses: u8,
    /// A key was applied since the last disagreeing digest.
    keyed: bool,
}

impl Check {
    /// The replica took a key.
    pub fn key_applied(&mut self) {
        self.keyed = true;
    }

    /// Compare the replica, after the digest's packet is applied, with the
    /// digest. An agreeing digest ends the run. A disagreeing one adds to
    /// the run when it is the first or a key came since the last.
    pub fn observe(&mut self, replica: &ActorSet, state_hash: u64, actor_count: u16) -> Verdict {
        let keyed = std::mem::take(&mut self.keyed);
        if replica.agrees_with(state_hash, actor_count) {
            self.misses = 0;
            return Verdict::Agrees;
        }
        if self.misses == 0 || keyed {
            self.misses = self.misses.saturating_add(1);
        }
        if self.misses >= DIVERGED_AFTER {
            Verdict::Diverged
        } else {
            Verdict::RequestKey
        }
    }

    /// Disagreeing digests in the current run.
    pub fn misses(&self) -> u8 {
        self.misses
    }
}

#[cfg(test)]
mod tests;
