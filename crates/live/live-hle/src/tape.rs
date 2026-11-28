//! The machine tape of a client film at the Live HLE boundary
//! (docs/game/film-format.md, "The machine tape").
//!
//! While a session records, every Live HLE entry the guest calls is kept
//! in order: its arguments, the value it returned, the calling thread's
//! `WSAGetLastError` when the call changed it, and every guest byte it
//! wrote. Winsock and XNet calls are gathered by `network::capture`; the
//! service's entries by `Guest::capture_hle_writes`. That is every receive
//! the guest makes (`recv` and `recvfrom` payloads and source addresses,
//! `select` and FIONREAD results, `XNetGetConnectStatus`, QoS results,
//! `XNetRandom`) and every Live answer.
//!
//! A replay serves call *n* of the tape as call *n* of the run and touches
//! no socket and no server. A call the tape does not hold, or one that
//! differs from the recorded call at its index (another entry, other
//! arguments), is a named error that stops the guest ([`TapeError`]); the
//! replay never falls back to live data.
use film::tape::{Answer, Call};
use std::sync::{Arc, Mutex, MutexGuard};

/// The most bytes a recording keeps in memory. A session past it keeps no
/// client film ([`Tape::spoiled`]).
pub const MAX_RECORDED_BYTES: usize = 512 << 20;

/// Why a replay could not answer a call.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TapeError {
    #[error(
        "film replay: call {index} ({name}) has no recorded answer: the tape holds {recorded} calls"
    )]
    NoAnswer {
        index: u64,
        name: &'static str,
        recorded: u64,
    },
    #[error(
        "film replay: call {index} is {name} {args:x?}, but the tape holds {recorded_name} {recorded_args:x?} there"
    )]
    Diverged {
        index: u64,
        name: &'static str,
        args: Vec<u32>,
        recorded_name: &'static str,
        recorded_args: Vec<u32>,
    },
    #[error("film replay: call {index} ({name}): {why}")]
    Unplayable {
        index: u64,
        name: &'static str,
        why: String,
    },
}

enum Inner {
    Record {
        calls: Vec<Call>,
        bytes: usize,
        spoiled: Option<String>,
    },
    Replay {
        calls: Vec<Call>,
        next: usize,
        failure: Option<TapeError>,
    },
}

/// One session's tape, recording or replaying ([`crate::Hle::set_tape`]).
pub struct Tape {
    inner: Mutex<Inner>,
}

fn size(call: &Call) -> usize {
    let answer = match &call.answer {
        Answer::Return(r) => r.writes.iter().map(|(_, b)| b.len() + 8).sum::<usize>() + 32,
        Answer::Stop(why) => why.len(),
    };
    16 + call.args.len() * 4 + answer
}

impl Tape {
    /// A tape that keeps every call from now on.
    pub fn recorder() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::Record {
                calls: Vec::new(),
                bytes: 0,
                spoiled: None,
            }),
        })
    }

    /// A tape that answers the guest with `calls`, in order.
    pub fn replayer(calls: Vec<Call>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::Replay {
                calls,
                next: 0,
                failure: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_replay(&self) -> bool {
        matches!(*self.lock(), Inner::Replay { .. })
    }

    /// Keep one call. A tape past [`MAX_RECORDED_BYTES`] stops keeping
    /// and says so.
    pub fn record(&self, call: Call) {
        let Inner::Record {
            calls,
            bytes,
            spoiled,
        } = &mut *self.lock()
        else {
            return;
        };
        if spoiled.is_some() {
            return;
        }
        *bytes += size(&call);
        if *bytes > MAX_RECORDED_BYTES {
            *spoiled = Some(format!(
                "the machine tape passed {} MiB at call {}",
                MAX_RECORDED_BYTES >> 20,
                calls.len()
            ));
            calls.clear();
            return;
        }
        calls.push(call);
    }

    /// Stop keeping calls: the tape cannot hold what a call did.
    pub fn spoil(&self, why: String) {
        if let Inner::Record { calls, spoiled, .. } = &mut *self.lock()
            && spoiled.is_none()
        {
            *spoiled = Some(format!("call {}: {why}", calls.len()));
            calls.clear();
        }
    }

    /// Why this recording keeps no tape, once it cannot.
    pub fn spoiled(&self) -> Option<String> {
        match &*self.lock() {
            Inner::Record { spoiled, .. } => spoiled.clone(),
            Inner::Replay { .. } => None,
        }
    }

    /// The calls recorded so far, or the calls a replay holds.
    pub fn calls(&self) -> Vec<Call> {
        match &*self.lock() {
            Inner::Record { calls, .. } | Inner::Replay { calls, .. } => calls.clone(),
        }
    }

    /// How many calls are recorded, or how many a replay has answered.
    pub fn position(&self) -> u64 {
        match &*self.lock() {
            Inner::Record { calls, .. } => calls.len() as u64,
            Inner::Replay { next, .. } => *next as u64,
        }
    }

    /// The error that stopped a replay.
    pub fn failure(&self) -> Option<TapeError> {
        match &*self.lock() {
            Inner::Replay { failure, .. } => failure.clone(),
            Inner::Record { .. } => None,
        }
    }

    /// The recorded call at the replay's position, when it is this entry
    /// with these arguments; the position moves on. Once a replay has
    /// failed, every later call fails with the same error.
    pub fn answer(&self, entry: u32, args: &[u32]) -> Result<Call, TapeError> {
        let Inner::Replay {
            calls,
            next,
            failure,
        } = &mut *self.lock()
        else {
            return Err(TapeError::NoAnswer {
                index: 0,
                name: crate::entries::name_at(entry),
                recorded: 0,
            });
        };
        if let Some(f) = failure {
            return Err(f.clone());
        }
        let index = *next as u64;
        let error = match calls.get(*next) {
            None => TapeError::NoAnswer {
                index,
                name: crate::entries::name_at(entry),
                recorded: calls.len() as u64,
            },
            Some(call) if call.entry == entry && call.args == args => {
                *next += 1;
                return Ok(call.clone());
            }
            Some(call) => TapeError::Diverged {
                index,
                name: crate::entries::name_at(entry),
                args: args.to_vec(),
                recorded_name: crate::entries::name_at(call.entry),
                recorded_args: call.args.clone(),
            },
        };
        *failure = Some(error.clone());
        Err(error)
    }

    /// A replay could not apply its answer at `index`: it stops there.
    pub fn fail(&self, error: TapeError) {
        if let Inner::Replay { failure, .. } = &mut *self.lock() {
            failure.get_or_insert(error);
        }
    }
}

/// Runtime permissions as the tape stores them (1 read, 2 write, 4
/// execute), and back.
pub(crate) fn permission_bits(p: runtime::guest::Permissions) -> u8 {
    use runtime::guest::Permissions as P;
    match p {
        P::NONE => 0,
        P::READ => 1,
        P::RW => 3,
        P::RX => 5,
        _ => 7,
    }
}

pub(crate) fn permissions(bits: u8) -> Option<runtime::guest::Permissions> {
    use runtime::guest::Permissions as P;
    Some(match bits {
        0 => P::NONE,
        1 => P::READ,
        3 => P::RW,
        5 => P::RX,
        7 => P::RWX,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
