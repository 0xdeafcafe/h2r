//! Ranked-game arbitration (docs/live/live-matchmaking.md §8.3). A round id
//! is made locally; registrations and reports are accepted and kept here,
//! owned, with no guest pointers. Nothing is sent to the server yet: the
//! server ranks from the games log, and checking a report against it waits
//! for a round to be linked to a game id.
use crate::{
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, guest::Access};
use std::collections::{BTreeSet, VecDeque};

/// The session id and round id the game passes by pointer.
const ROUND_SIZE: u32 = 16;
/// One stat procedure record of a report.
const PROCEDURE_SIZE: u32 = 0x88;
/// Most procedures in one report (the XDK's limit).
const MAX_PROCEDURES: u32 = 1000;
/// Most registrations and reports kept.
const MAX_KEPT: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    CreateRoundId,
    Register,
    Report,
}

/// A round as the game names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArbitrationRound {
    pub session_id: u64,
    pub round_id: u64,
}

/// What the game told arbitration, in call order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arbitration {
    Registered {
        round: ArbitrationRound,
        /// The duration-like word the game passes (not yet decoded).
        duration: u32,
        flags: u32,
    },
    Reported {
        round: ArbitrationRound,
        flags: u32,
        /// Each stat procedure's id (0x8001, 0x8003, 0x8007 seen); their
        /// data is not decoded yet.
        procedures: Vec<u16>,
    },
}

#[derive(Default)]
pub(crate) struct Arbiter {
    tasks: BTreeSet<u32>,
    kept: VecDeque<Arbitration>,
}

type Result<T> = std::result::Result<T, Error>;

impl Arbiter {
    pub fn snapshot(&self) -> Vec<Arbitration> {
        self.kept.iter().cloned().collect()
    }
    pub fn clear_tasks(&mut self) {
        self.tasks.clear();
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.tasks.remove(&h)
    }
    /// Registrations and reports complete at once.
    pub fn poll(&self, h: u32) -> Option<u32> {
        self.tasks.contains(&h).then_some(TASK_SUCCESS)
    }
    pub fn invoke(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        storage: &mut Storage,
    ) -> Result<u32> {
        match op {
            Op::CreateRoundId => {
                m.check(a[0], 8, Access::Write, 1)?;
                let id = loop {
                    let Ok(bytes) = crate::network::random(8) else {
                        return Ok(NO_ACCEPTED_INVITE); // E_FAIL
                    };
                    let id = u64::from_le_bytes(bytes.try_into().expect("eight bytes"));
                    if id != 0 {
                        break id;
                    }
                };
                m.write_bytes(a[0], &id.to_le_bytes(), 1)?;
                Ok(OK)
            }
            Op::Register => {
                // (round, duration, flags, event, task)
                if a[3] != 0 {
                    return Ok(INVALID_ARGUMENT);
                }
                let round = round(m, a[0])?;
                self.accept(
                    m,
                    storage,
                    a[4],
                    Arbitration::Registered {
                        round,
                        duration: a[1],
                        flags: a[2],
                    },
                )
            }
            Op::Report => {
                // (round, count, procedures, 0, flags, event, task)
                if a[3] != 0 || a[5] != 0 || a[1] > MAX_PROCEDURES {
                    return Ok(INVALID_ARGUMENT);
                }
                let round = round(m, a[0])?;
                m.check(a[2], (a[1] * PROCEDURE_SIZE) as usize, Access::Read, 1)?;
                let procedures = (0..a[1])
                    .map(|i| m.u16(a[2] + i * PROCEDURE_SIZE))
                    .collect::<std::result::Result<_, _>>()?;
                self.accept(
                    m,
                    storage,
                    a[6],
                    Arbitration::Reported {
                        round,
                        flags: a[4],
                        procedures,
                    },
                )
            }
        }
    }
    fn accept(
        &mut self,
        m: &mut Guest,
        storage: &mut Storage,
        out: u32,
        record: Arbitration,
    ) -> Result<u32> {
        m.check(out, 4, Access::Write, 1)?;
        let Some(h) = storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        m.set_u32(out, h)?;
        self.tasks.insert(h);
        if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
            eprintln!("live arbitration: {record:?}");
        }
        if self.kept.len() == MAX_KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back(record);
        Ok(OK)
    }
}

fn round(m: &Guest, p: u32) -> Result<ArbitrationRound> {
    m.check(p, ROUND_SIZE as usize, Access::Read, 1)?;
    Ok(ArbitrationRound {
        session_id: m.u64(p)?,
        round_id: m.u64(p + 8)?,
    })
}
