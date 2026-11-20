//! `sim.hashes` (docs/graphics/framescene.md, section 9.3): the running
//! game's post-tick digests, in the rows sim mode writes to its report
//! (`state_hashes_v1`), so two rendered runs can be compared tick by tick.
//!
//! The host's tick observer computes each tick's digest as sim mode does
//! (boot's `agent_frames`) and [`HashLog::record`]s it; the log keeps the
//! last [`HashesReply::KEEP`] rows (24 bytes each, about 2.6 MB), dropping
//! the oldest, and starts again when the tick goes back or jumps, as the
//! sim report's tally does.

use std::collections::VecDeque;
use std::sync::Mutex;

use agent_proto::sim::hashes::{HashRow, HashesParams, HashesReply};
use agent_proto::sim::method;
use agent_proto::{RpcError, code};

use crate::{Access, Agent};

/// A tick more than this after the last starts the series again (the sim
/// report tally's `JUMP_TICKS`).
pub const JUMP_TICKS: u32 = 30;

/// The rows. One per process ([`HASHES`]); tests make their own.
pub struct HashLog {
    ring: Mutex<Ring>,
}

struct Ring {
    keep: usize,
    rows: VecDeque<(u32, u64, Option<u64>)>,
    last: Option<u32>,
    dropped: u64,
    resets: u64,
}

/// The game's.
pub static HASHES: HashLog = HashLog::new(HashesReply::KEEP);

impl HashLog {
    /// Keeps at most `keep` rows (at least one).
    pub const fn new(keep: usize) -> HashLog {
        HashLog {
            ring: Mutex::new(Ring {
                keep: if keep == 0 { 1 } else { keep },
                rows: VecDeque::new(),
                last: None,
                dropped: 0,
                resets: 0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The digest of the tick just ended: `gameplay` and, under
    /// `impl.sim.arena-hash`, `arena`. The latest wins for a tick seen
    /// twice; a tick before the last, or more than [`JUMP_TICKS`] after
    /// it, starts again.
    pub fn record(&self, tick: u32, gameplay: u64, arena: Option<u64>) {
        let mut r = self.lock();
        if let Some(last) = r.last {
            if tick == last {
                if let Some(row) = r.rows.back_mut() {
                    *row = (tick, gameplay, arena);
                }
                return;
            }
            if tick < last || tick > last.saturating_add(JUMP_TICKS) {
                r.rows.clear();
                r.dropped = 0;
                r.resets += 1;
            }
        }
        if r.rows.len() >= r.keep {
            r.rows.pop_front();
            r.dropped += 1;
        }
        r.rows.push_back((tick, gameplay, arena));
        r.last = Some(tick);
    }

    /// The rows from `p.from_tick` on, at most `p.max`.
    pub fn reply(&self, p: &HashesParams) -> HashesReply {
        let r = self.lock();
        let from = p.from_tick.unwrap_or(0);
        let max = p.max.unwrap_or(usize::MAX);
        let mut matching = r.rows.iter().filter(|row| row.0 >= from);
        let rows: Vec<HashRow> = matching
            .by_ref()
            .take(max)
            .map(|&(tick, gameplay, arena)| row(tick, gameplay, arena))
            .collect();
        HashesReply {
            rows,
            dropped: r.dropped,
            resets: r.resets,
            truncated: matching.next().is_some(),
        }
    }
}

/// One row, as the sim report writes it: 16 hex digits each.
pub fn row(tick: u32, gameplay: u64, arena: Option<u64>) -> HashRow {
    HashRow {
        tick,
        gameplay: format!("{gameplay:016x}"),
        arena: arena.map(|h| format!("{h:016x}")),
    }
}

/// Adds `sim.hashes` over `log`.
pub fn install(agent: &Agent, log: &'static HashLog) {
    agent.add_method(method::HASHES, Access::Read, move |v| {
        let p: HashesParams = if v.is_null() {
            HashesParams::default()
        } else {
            serde_json::from_value(v)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
        };
        serde_json::to_value(log.reply(&p))
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use sim_report::metrics::{End, Finish, Tally, TickSample};

    /// The same observations, through the sim report's tally and through
    /// the log: the same rows, byte for byte in JSON.
    #[test]
    fn rows_are_the_sim_reports_state_hashes() {
        let ticks: [(u32, u64, Option<u64>); 5] = [
            (5, 1, Some(0xfeed)),
            (5, 2, Some(0xbeef)),
            (6, u64::MAX, None),
            (7, 0x0123_4567_89ab_cdef, Some(0)),
            (8, 9, None),
        ];
        let mut tally = Tally::new();
        let log = HashLog::new(100);
        for (tick, gameplay, arena) in ticks {
            tally.tick_hashed(
                TickSample {
                    tick,
                    ..Default::default()
                },
                gameplay,
                arena,
            );
            log.record(tick, gameplay, arena);
        }
        let report = serde_json::to_value(tally.report(Finish::bare(End::Stopped))).unwrap();
        let ours = serde_json::to_value(log.reply(&HashesParams::default())).unwrap();
        assert_eq!(ours["rows"], report["state_hashes_v1"]);
        assert_eq!(ours["rows"].as_array().unwrap().len(), 4, "tick 5 once");
        assert_eq!(ours["rows"][0]["gameplay"], "0000000000000002");
        assert!(ours["rows"][1].get("arena").is_none());
        // And they read back as the report's type.
        let back: Vec<sim_report::metrics::TickHash> =
            serde_json::from_value(ours["rows"].clone()).unwrap();
        assert_eq!(back.len(), 4);
    }

    #[test]
    fn from_tick_and_max_pick_rows_and_say_when_more_matched() {
        let log = HashLog::new(100);
        for t in 10..20 {
            log.record(t, u64::from(t), None);
        }
        let r = log.reply(&HashesParams {
            from_tick: Some(15),
            max: None,
        });
        assert_eq!(r.rows.first().unwrap().tick, 15);
        assert_eq!(r.rows.len(), 5);
        assert!(!r.truncated);
        let r = log.reply(&HashesParams {
            from_tick: Some(15),
            max: Some(2),
        });
        assert_eq!(
            r.rows.iter().map(|r| r.tick).collect::<Vec<_>>(),
            [15, 16]
        );
        assert!(r.truncated);
    }

    #[test]
    fn memory_is_bounded_and_a_rewind_or_jump_starts_again() {
        let log = HashLog::new(4);
        for t in 0..10 {
            log.record(t, 1, None);
        }
        let r = log.reply(&HashesParams::default());
        assert_eq!(
            r.rows.iter().map(|r| r.tick).collect::<Vec<_>>(),
            [6, 7, 8, 9]
        );
        assert_eq!((r.dropped, r.resets), (6, 0));
        // A new map: the tick goes back.
        log.record(0, 2, None);
        let r = log.reply(&HashesParams::default());
        assert_eq!(r.rows.len(), 1);
        assert_eq!((r.dropped, r.resets), (0, 1));
        // A snapshot load far ahead.
        log.record(1 + JUMP_TICKS + 1, 3, None);
        let r = log.reply(&HashesParams::default());
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.resets, 2);
    }

    #[test]
    fn the_method_answers_the_rows() {
        let log: &'static HashLog = Box::leak(Box::new(HashLog::new(10)));
        log.record(3, 0xabc, Some(0xdef));
        let agent = Agent::new();
        install(&agent, log);
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let conn = agent.connect(tx);
        struct Nothing;
        impl crate::Reader for Nothing {
            fn status(&mut self) -> crate::GameStatus {
                crate::GameStatus::default()
            }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(vec![])
            }
        }
        let call = |p: Value| -> Value {
            agent.submit(
                conn,
                &json!({"jsonrpc": "2.0", "id": 1, "method": method::HASHES, "params": p})
                    .to_string(),
            );
            agent.between_slices(&mut Nothing, 0);
            serde_json::from_str(
                &rx.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap(),
            )
            .unwrap()
        };
        let r = call(Value::Null);
        assert_eq!(
            r["result"]["rows"],
            json!([{"tick": 3, "gameplay": "0000000000000abc", "arena": "0000000000000def"}])
        );
        let r = call(json!({"from_tik": 3}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
    }
}
