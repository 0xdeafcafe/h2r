//! `state.*` (docs/runtime/snapshots.md, phase 5): machine snapshots.
//!
//! `state.list` and `state.drop` touch only the store ([`Snapshots`]) and
//! are answered at once. `state.save`, `state.load` and `state.fork` queue
//! a job. While the game ticks, a job becomes due at the next tick, and
//! [`Agent::on_tick`] asks the tick observer to end the slice there
//! (`Runtime::request_preempt`); the boot loop then calls
//! [`Agent::run_snapshots`] between slices with the machine at hand
//! ([`SnapshotMachine`]). A job that finds a blocker waits for the next
//! tick and tries again, at most `max_ticks` times, then answers
//! [`code::BUSY`] with the blockers. With no ticks (loading, or no tick
//! observer) every slice boundary is a try.
//!
//! The machine is driven without the agent's lock held: the plan's
//! feature sections include the agent's own ([`Agent::save_state`]).

use std::collections::VecDeque;
use std::time::Instant;

use agent_proto::state::{
    self as proto, Busy, Dropped, ForkParams, Forked, List, LoadParams, Loaded, NameParams,
    SaveParams, Saved,
};
use agent_proto::{At, RpcError, code};
use serde_json::Value;

use super::{Agent, ConnId, State, params, to_value};

/// The snapshot store, for the methods that need no machine: list, drop,
/// and the name checks a save or load makes before it waits for a tick.
/// Called with the agent's state locked: file system only.
pub trait Snapshots: Send {
    fn list(&mut self) -> Result<List, RpcError>;
    fn remove(&mut self, name: &str) -> Result<(), RpcError>;
    fn exists(&mut self, name: &str) -> bool;
}

/// The running machine, between slices (boot's snapshot glue).
pub trait SnapshotMachine {
    /// Everything that keeps a save or load from running now, as
    /// `owner: why`; empty when nothing does.
    fn blockers(&mut self) -> Vec<String>;
    /// Saves the machine as `name`. `at` is the boundary it is taken at.
    /// `cow` also writes the raw RAM image later loads may map
    /// copy-on-write.
    fn save(&mut self, name: &str, replace: bool, at: &At, cow: bool) -> Result<Saved, RpcError>;
    /// Loads `name` into the machine, in the documented order, and checks
    /// the RAM hash. `cow` maps the saved RAM image copy-on-write in place
    /// of decoding it in.
    fn load(&mut self, name: &str, cow: bool) -> Result<Loaded, RpcError>;
}

#[derive(Debug, Clone, PartialEq)]
enum Op {
    Save {
        name: String,
        replace: bool,
        cow: bool,
    },
    Load {
        name: String,
        cow: bool,
    },
    Fork {
        from: String,
        name: String,
        replace: bool,
    },
}

pub(crate) struct Job {
    conn: ConnId,
    id: Value,
    op: Op,
    /// Tries allowed.
    max: u64,
    /// Tries made that found a blocker.
    tries: u64,
    /// A tick has passed since it was queued or last tried.
    due: bool,
    blockers: Vec<String>,
}

impl Job {
    pub(crate) fn conn(&self) -> ConnId {
        self.conn
    }
}

enum Outcome {
    Retry(Job),
    Done(Job, Result<Value, RpcError>),
}

pub(crate) fn is_method(m: &str) -> bool {
    proto::method::ALL.contains(&m)
}

fn bad_name(name: &str) -> Option<RpcError> {
    snapshot::store::check_name(name)
        .err()
        .map(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn max_ticks(n: Option<u64>) -> Result<u64, RpcError> {
    let n = n.unwrap_or(proto::DEFAULT_TICKS);
    if (1..=proto::MAX_TICKS).contains(&n) {
        Ok(n)
    } else {
        Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("max_ticks: 1 to {}", proto::MAX_TICKS),
        ))
    }
}

impl Agent {
    /// Turns `state.*` on, over `store`. Without it they answer
    /// `UNAVAILABLE`.
    pub fn set_snapshots(&self, store: Box<dyn Snapshots>) {
        self.lock().snapshots = Some(store);
    }

    /// Whether a save or load waits for the boot loop
    /// ([`Agent::run_snapshots`]).
    pub fn snapshot_pending(&self) -> bool {
        !self.lock().snap_jobs.is_empty()
    }

    /// Between slices, on the game's thread: runs the save and load jobs
    /// that are due, on `machine`. Call it after
    /// [`Agent::between_slices`], with nothing of the game's locked.
    pub fn run_snapshots(&self, machine: &mut dyn SnapshotMachine) {
        let (jobs, at) = {
            let mut s = self.lock();
            if s.shutdown || s.snap_jobs.is_empty() {
                return;
            }
            let idle = s.no_tick_hook || !s.ticking();
            let (due, wait): (VecDeque<Job>, VecDeque<Job>) = std::mem::take(&mut s.snap_jobs)
                .into_iter()
                .partition(|j| j.due || idle);
            s.snap_jobs = wait;
            (due, s.at())
        };
        if jobs.is_empty() {
            return;
        }
        let mut outcomes = Vec::new();
        for mut job in jobs {
            let blockers = machine.blockers();
            if !blockers.is_empty() {
                job.tries += 1;
                job.blockers = blockers;
                job.due = false;
                outcomes.push(Outcome::Retry(job));
                continue;
            }
            let out = run(machine, &job, &at);
            outcomes.push(Outcome::Done(job, out));
        }
        let mut s = self.lock();
        let mut retry = VecDeque::new();
        for o in outcomes {
            match o {
                Outcome::Retry(job) if job.tries >= job.max => {
                    let at = s.at();
                    let mut e = RpcError::new(
                        code::BUSY,
                        format!(
                            "not quiescent at {} slice boundaries: {}",
                            job.tries,
                            job.blockers.join("; ")
                        ),
                    );
                    e.data = Some(to_value(Busy {
                        at,
                        blockers: job.blockers,
                        tries: job.tries,
                    }));
                    s.send(job.conn, agent_proto::Response::err(job.id, e));
                }
                Outcome::Retry(job) => retry.push_back(job),
                Outcome::Done(job, out) => {
                    let at = to_value(s.at());
                    let r = match out {
                        Ok(mut v) => {
                            if let Value::Object(o) = &mut v {
                                o.entry("at").or_insert(at);
                            }
                            agent_proto::Response::ok(job.id, v)
                        }
                        Err(e) => agent_proto::Response::err(job.id, e),
                    };
                    s.send(job.conn, r);
                }
            }
        }
        retry.extend(std::mem::take(&mut s.snap_jobs));
        s.snap_jobs = retry;
        s.prune();
    }
}

/// One job on a quiescent machine.
fn run(machine: &mut dyn SnapshotMachine, job: &Job, at: &At) -> Result<Value, RpcError> {
    let t = Instant::now();
    let ms = |t: Instant| (t.elapsed().as_secs_f64() * 1e4).round() / 10.0;
    match &job.op {
        Op::Save { name, replace, cow } => {
            let mut saved = machine.save(name, *replace, at, *cow)?;
            saved.retries = job.tries;
            saved.ms = ms(t);
            Ok(to_value(saved))
        }
        Op::Load { name, cow } => {
            let mut loaded = machine.load(name, *cow)?;
            loaded.retries = job.tries;
            loaded.ms = ms(t);
            Ok(to_value(loaded))
        }
        Op::Fork {
            from,
            name,
            replace,
        } => {
            let mut loaded = machine.load(from, false)?;
            loaded.retries = job.tries;
            loaded.ms = ms(t);
            let t = Instant::now();
            let mut saved = machine.save(name, *replace, at, false).map_err(|mut e| {
                e.message = format!("loaded {from}, but saving it as {name}: {}", e.message);
                e
            })?;
            saved.ms = ms(t);
            Ok(to_value(Forked { loaded, saved }))
        }
    }
}

impl State {
    /// `state.*`. `None`: queued, answered by [`Agent::run_snapshots`].
    pub(super) fn state_method(
        &mut self,
        conn: ConnId,
        id: Option<Value>,
        m: &str,
        p: Value,
    ) -> Option<Result<Value, RpcError>> {
        if m != proto::method::LIST && self.controller.is_some_and(|c| c != conn) {
            return Some(Err(RpcError::new(
                code::NOT_CONTROLLER,
                "another connection holds control",
            )));
        }
        let Some(store) = self.snapshots.as_mut() else {
            return Some(Err(RpcError::new(
                code::UNAVAILABLE,
                "this game has no machine snapshots (boot's snapshot glue is not installed)",
            )));
        };
        let op = match m {
            proto::method::LIST => return Some(store.list().map(to_value)),
            proto::method::DROP => {
                return Some(params::<NameParams>(p).and_then(|np| {
                    store.remove(&np.name)?;
                    Ok(to_value(Dropped { name: np.name }))
                }));
            }
            proto::method::SAVE => params::<SaveParams>(p).and_then(|sp| {
                Ok((
                    Op::Save {
                        name: sp.name,
                        replace: sp.replace,
                        cow: sp.cow,
                    },
                    max_ticks(sp.max_ticks)?,
                ))
            }),
            proto::method::LOAD => params::<LoadParams>(p).and_then(|lp| {
                Ok((
                    Op::Load {
                        name: lp.name,
                        cow: lp.cow,
                    },
                    max_ticks(lp.max_ticks)?,
                ))
            }),
            _ => params::<ForkParams>(p).and_then(|fp| {
                Ok((
                    Op::Fork {
                        from: fp.from,
                        name: fp.name,
                        replace: fp.replace,
                    },
                    max_ticks(fp.max_ticks)?,
                ))
            }),
        };
        let (op, max) = match op {
            Ok(o) => o,
            Err(e) => return Some(Err(e)),
        };
        let (load, save) = match &op {
            Op::Save { name, replace, .. } => (None, Some((name, *replace))),
            Op::Load { name, .. } => (Some(name), None),
            Op::Fork {
                from,
                name,
                replace,
            } => (Some(from), Some((name, *replace))),
        };
        for name in load.into_iter().chain(save.map(|(n, _)| n)) {
            if let Some(e) = bad_name(name) {
                return Some(Err(e));
            }
        }
        if let Some(name) = load
            && !store.exists(name)
        {
            return Some(Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("no snapshot named {name:?} for this build (state.list)"),
            )));
        }
        if let Some((name, false)) = save
            && store.exists(name)
        {
            return Some(Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("a snapshot named {name:?} exists; \"replace\": true overwrites it"),
            )));
        }
        let id = id?;
        self.snap_jobs.push_back(Job {
            conn,
            id,
            op,
            max,
            tries: 0,
            due: false,
            blockers: Vec::new(),
        });
        None
    }

    /// At a tick: every queued job is due at this slice's end. Whether
    /// any is (the tick observer then ends the slice early).
    pub(super) fn arm_snapshots(&mut self) -> bool {
        for j in &mut self.snap_jobs {
            j.due = true;
        }
        !self.snap_jobs.is_empty()
    }

    /// The game is exiting: every queued job fails.
    pub(super) fn fail_snapshots(&mut self) {
        for j in std::mem::take(&mut self.snap_jobs) {
            self.send(
                j.conn,
                agent_proto::Response::err(
                    j.id,
                    RpcError::new(code::SHUTDOWN, "the game is exiting"),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests;
