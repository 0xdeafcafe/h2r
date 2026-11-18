//! Ranked-game arbitration (docs/live/live-matchmaking.md §8.4). A round
//! id is made locally. Register, ExtendRound and Report are kept here,
//! owned, with no guest pointers, and posted to the server as every
//! account signed in on this console, through the Live worker: the task
//! waits for its answer, never the call. With no server (System Link,
//! offline) they are only kept, and complete at once.
use crate::{
    client::{Pending, Service, Session},
    state::{Error, Storage},
    status::*,
};
use live_proto::{arbitration as wire, sign::hex};
use runtime::{Guest, guest::Access};
use std::collections::{BTreeMap, VecDeque};

/// The session id and round id the game passes by pointer.
const ROUND_SIZE: u32 = 16;
/// One stat procedure record of a report.
const PROCEDURE_SIZE: u32 = 0x88;
/// Most procedures in one report (the XDK's limit).
const MAX_PROCEDURES: u32 = wire::MAX_PROCEDURES as u32;
/// Most registrations and reports kept.
const MAX_KEPT: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    CreateRoundId,
    Register,
    Extend,
    Report,
}

/// A round as the game names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArbitrationRound {
    pub session_id: u64,
    pub round_id: u64,
}

impl ArbitrationRound {
    /// The session id as the API writes it: the game's 8 bytes in order.
    pub fn session_hex(&self) -> String {
        hex(&self.session_id.to_le_bytes())
    }
    pub fn round_hex(&self) -> String {
        hex(&self.round_id.to_le_bytes())
    }
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
    Extended {
        round: ArbitrationRound,
        duration: u32,
    },
    Reported {
        round: ArbitrationRound,
        flags: u32,
        /// Each stat procedure's id (0x8001, 0x8003, 0x8007 seen); their
        /// data is not decoded yet.
        procedures: Vec<u16>,
    },
}

impl Arbitration {
    fn round(&self) -> ArbitrationRound {
        match self {
            Self::Registered { round, .. }
            | Self::Extended { round, .. }
            | Self::Reported { round, .. } => *round,
        }
    }
    /// The server call: its path under `/arbitration/` and its body.
    fn call(&self) -> (String, String) {
        let r = self.round();
        let (action, body) = match self {
            Self::Registered {
                duration, flags, ..
            } => (
                "register",
                serde_json::to_string(&wire::Register {
                    duration: *duration,
                    flags: *flags,
                }),
            ),
            Self::Extended { duration, .. } => (
                "extend",
                serde_json::to_string(&wire::Extend {
                    duration: *duration,
                }),
            ),
            Self::Reported {
                flags, procedures, ..
            } => (
                "report",
                serde_json::to_string(&wire::Report {
                    flags: *flags,
                    procedures: procedures.clone(),
                }),
            ),
        };
        (
            format!("{}/{}/{action}", r.session_hex(), r.round_hex()),
            body.expect("serializes"),
        )
    }
}

pub(crate) struct Context<'a> {
    pub storage: &'a mut Storage,
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
    /// The session this console advertises, for the log.
    pub advertising: Option<[u8; 8]>,
}

enum Task {
    Waiting(Pending),
    Done(u32),
}

#[derive(Default)]
pub(crate) struct Arbiter {
    tasks: BTreeMap<u32, Task>,
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
        self.tasks.remove(&h).is_some()
    }
    /// 0 while the server has not answered; then success, or the
    /// server's failure as the XDK reports it.
    pub fn poll(&mut self, h: u32) -> Option<u32> {
        let task = self.tasks.get_mut(&h)?;
        if let Task::Waiting(p) = task
            && let Some(reply) = p.take()
        {
            *task = Task::Done(match reply {
                Ok(_) => TASK_SUCCESS,
                Err(hr) => hr,
            });
        }
        Some(match task {
            Task::Waiting(_) => 0,
            Task::Done(hr) => *hr,
        })
    }
    pub fn invoke(&mut self, op: Op, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Result<u32> {
        let record = match op {
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
                return Ok(OK);
            }
            Op::Register => {
                // (round, duration, flags, event, task)
                if a[3] != 0 {
                    return Ok(INVALID_ARGUMENT);
                }
                m.check(a[4], 4, Access::Write, 1)?;
                Arbitration::Registered {
                    round: round(m, a[0])?,
                    duration: a[1],
                    flags: a[2],
                }
            }
            Op::Extend => {
                // (round, duration, event, task)
                if a[2] != 0 {
                    return Ok(INVALID_ARGUMENT);
                }
                m.check(a[3], 4, Access::Write, 1)?;
                Arbitration::Extended {
                    round: round(m, a[0])?,
                    duration: a[1],
                }
            }
            Op::Report => {
                // (round, count, procedures, 0, flags, event, task)
                if a[3] != 0 || a[5] != 0 || a[1] > MAX_PROCEDURES {
                    return Ok(INVALID_ARGUMENT);
                }
                m.check(a[6], 4, Access::Write, 1)?;
                let round = round(m, a[0])?;
                m.check(a[2], (a[1] * PROCEDURE_SIZE) as usize, Access::Read, 1)?;
                let procedures = (0..a[1])
                    .map(|i| m.u16(a[2] + i * PROCEDURE_SIZE))
                    .collect::<std::result::Result<_, _>>()?;
                Arbitration::Reported {
                    round,
                    flags: a[4],
                    procedures,
                }
            }
        };
        let out = match op {
            Op::Register => a[4],
            Op::Extend => a[3],
            _ => a[6],
        };
        if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
            // The game-run check (live-matchmaking.md §8.4): the round's
            // session id should be the session this console advertises or
            // joined.
            let r = record.round();
            eprintln!(
                "live arbitration: session {} round {} (advertising {}): {record:?}",
                r.session_hex(),
                r.round_hex(),
                ctx.advertising.map_or("none".into(), |id| hex(&id)),
            );
        }
        let task = match (ctx.service, ctx.session) {
            (Some(service), Some(session)) => {
                let (path, body) = record.call();
                match service.arbitrate(session, path, body) {
                    Ok(p) => Task::Waiting(p),
                    Err(hr) => return Ok(hr),
                }
            }
            _ => Task::Done(TASK_SUCCESS),
        };
        let Some(h) = ctx.storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        m.set_u32(out, h)?;
        self.tasks.insert(h, task);
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
