//! Ordered host commands for Forge and Firefight.
//!
//! See `docs/game/ai-networking.md` for the session design.

pub use content::Sha256;
use serde::{Deserialize, Serialize};

/// A command accepted by the host and scheduled for an apply tick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry<Op> {
    pub seq: u32,
    pub tick: u32,
    /// The authenticated editor for Forge. Director commands omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub op: Op,
    pub prev_hash: Sha256,
}

impl<Op: Serialize> Entry<Op> {
    /// The UTF-8 form of the entire entry, using the integer-only canonical
    /// JSON encoding shared with Forge F1.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Error> {
        let value = serde_json::to_value(self)?;
        Ok(content::canonicalize(&value)?.into_bytes())
    }

    pub fn hash(&self) -> Result<Sha256, Error> {
        Ok(Sha256::of(&self.canonical_bytes()?))
    }
}

/// A position in the host log. At sequence zero the hash identifies the
/// starting document or session and the tick is zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogHead {
    pub seq: u32,
    pub tick: u32,
    pub hash: Sha256,
}

impl LogHead {
    pub fn genesis(hash: Sha256) -> Self {
        Self {
            seq: 0,
            tick: 0,
            hash,
        }
    }

    pub fn next_seq(self) -> Result<u32, Error> {
        self.seq.checked_add(1).ok_or(Error::SequenceExhausted)
    }

    fn advance<Op: Serialize>(self, entry: &Entry<Op>) -> Result<Self, Error> {
        let expected = self.next_seq()?;
        if entry.seq != expected {
            return Err(Error::Sequence {
                expected,
                actual: entry.seq,
            });
        }
        if entry.prev_hash != self.hash {
            return Err(Error::Predecessor { seq: entry.seq });
        }
        if entry.tick < self.tick {
            return Err(Error::TickBackwards {
                seq: entry.seq,
                previous: self.tick,
                actual: entry.tick,
            });
        }
        Ok(Self {
            seq: entry.seq,
            tick: entry.tick,
            hash: entry.hash()?,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the initial tick must be zero, got {0}")]
    InitialTick(u32),
    #[error("host log sequence exhausted")]
    SequenceExhausted,
    #[error("expected host log sequence {expected}, got {actual}")]
    Sequence { expected: u32, actual: u32 },
    #[error("predecessor hash differs at host log sequence {seq}")]
    Predecessor { seq: u32 },
    #[error("tick goes backwards at host log sequence {seq}: {previous} to {actual}")]
    TickBackwards {
        seq: u32,
        previous: u32,
        actual: u32,
    },
    #[error("host log head differs: expected {expected:?}, got {actual:?}")]
    HeadMismatch { expected: LogHead, actual: LogHead },
    #[error("could not serialize a host log entry: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not canonicalize a host log entry: {0}")]
    Canonical(#[from] content::NotInteger),
}

/// An append-only log, retaining entries after its starting checkpoint.
#[derive(Debug)]
pub struct Log<Op> {
    start: LogHead,
    head: LogHead,
    entries: Vec<Entry<Op>>,
}

impl<Op> Log<Op> {
    /// Start an empty log at genesis or a trusted checkpoint.
    pub fn new(start: LogHead) -> Result<Self, Error> {
        if start.seq == 0 && start.tick != 0 {
            return Err(Error::InitialTick(start.tick));
        }
        Ok(Self {
            start,
            head: start,
            entries: Vec::new(),
        })
    }

    pub fn head(&self) -> LogHead {
        self.head
    }

    /// Return the retained entries strictly after `seq`, in apply order.
    pub fn since(&self, seq: u32) -> &[Entry<Op>] {
        let offset = seq.saturating_sub(self.start.seq) as usize;
        &self.entries[offset.min(self.entries.len())..]
    }
}

impl<Op: Serialize> Log<Op> {
    /// Check and append the next entry, returning its head. Entries at the
    /// same tick retain their sequence order.
    pub fn push(&mut self, entry: Entry<Op>) -> Result<LogHead, Error> {
        let head = self.head.advance(&entry)?;
        self.entries.push(entry);
        self.head = head;
        Ok(head)
    }

    /// Recompute the retained chain and match it against a trusted head.
    /// Comparing the final head also detects truncation and changes to the
    /// last entry.
    pub fn verify(&self, expected: LogHead) -> Result<(), Error> {
        let mut actual = self.start;
        for entry in &self.entries {
            actual = actual.advance(entry)?;
        }
        if actual != expected {
            return Err(Error::HeadMismatch { expected, actual });
        }
        Ok(())
    }
}
