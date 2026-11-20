//! `trace.*` and `flight.*`: per-tick rows of chosen objects, sampled
//! inside the game at every tick boundary (docs/runtime/agent-api.md,
//! "Tick traces and the flight recorder").
//!
//! A trace streams one JSON line per tick ([`TickRow`]) to a file until
//! `trace.stop`. The flight recorder keeps the last N seconds of the same
//! rows, plus the controller reads of each tick, in memory, and writes
//! them out on `flight.dump`. [`show`] prints a file's rows back, so a
//! stretch can be stepped through as data (`h2 agent flight show`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::At;

/// Method names. Listed by `session.methods` wherever the agent runs.
pub mod method {
    /// Start streaming per-tick rows to a file ([`super::TraceParams`]).
    pub const TRACE_START: &str = "trace.start";
    /// Stop it, flushing the file ([`super::TraceStatus`]).
    pub const TRACE_STOP: &str = "trace.stop";
    /// The running (or last) trace's counts.
    pub const TRACE_STATUS: &str = "trace.status";
    /// Keep the last N seconds of rows and input in memory
    /// ([`super::FlightParams`]).
    pub const FLIGHT_START: &str = "flight.start";
    /// Write the ring to a file ([`super::DumpParams`]); it keeps running.
    pub const FLIGHT_DUMP: &str = "flight.dump";
    /// Stop it and drop the ring.
    pub const FLIGHT_STOP: &str = "flight.stop";
    /// Whether it runs, and what the ring holds.
    pub const FLIGHT_STATUS: &str = "flight.status";

    pub const ALL: &[&str] = &[
        TRACE_START,
        TRACE_STOP,
        TRACE_STATUS,
        FLIGHT_START,
        FLIGHT_DUMP,
        FLIGHT_STOP,
        FLIGHT_STATUS,
    ];
}

/// Game ticks per second (Halo 2 ticks at 30 Hz): `seconds` to rows.
pub const TICKS_PER_SECOND: u32 = 30;

/// `trace.start`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceParams {
    /// Object ids (`player.0`, `actor.e1230045`), 1 to [`MAX_OBJECTS`].
    pub objects: Vec<String>,
    /// [`crate::OBJECT_FIELDS`] names to keep (`id` and `kind` always
    /// are); all of them when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    /// Where to write; the cache directory's `traces/trace-<ms>.jsonl`
    /// when absent. An existing file is replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// At most this many objects per trace or flight recorder.
pub const MAX_OBJECTS: usize = 64;

/// `flight.start`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlightParams {
    pub objects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    /// How much to keep (default [`FlightParams::DEFAULT_SECONDS`], at most
    /// [`FlightParams::MAX_SECONDS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u32>,
}

impl FlightParams {
    pub const DEFAULT_SECONDS: u32 = 30;
    pub const MAX_SECONDS: u32 = 600;
}

/// `flight.dump`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DumpParams {
    /// Where to write; the cache directory's `flights/flight-<ms>.jsonl`
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `trace.start`, `trace.stop` and `trace.status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceStatus {
    pub at: At,
    /// Still writing rows.
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub objects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    /// Rows written: one per tick since the start.
    pub rows: u64,
    /// Bytes written.
    pub bytes: u64,
    /// The game ticks of the first and last row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_tick: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tick: Option<u64>,
    /// Writing failed (the trace stopped there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `flight.start`, `flight.stop`, `flight.status` and `flight.dump`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlightStatus {
    pub at: At,
    pub active: bool,
    pub objects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    pub seconds: u32,
    /// Rows the ring holds at most (`seconds` × 30).
    pub capacity: usize,
    /// Rows in the ring now.
    pub rows: usize,
    /// Their JSON bytes (the ring keeps at most 64 MiB, dropping the
    /// oldest rows first).
    pub bytes: usize,
    /// Ticks seen since the start (older ones have left the ring).
    pub ticks: u64,
    /// The game ticks of the oldest and newest row in the ring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_tick: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tick: Option<u64>,
    /// `flight.dump` only: the file written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One tick, as a trace or flight file line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TickRow {
    /// The game's own tick (the stamp's `tick`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    /// The agent's tick count: never resets, one more every row.
    pub ticks: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Each object asked for that is in the world, trimmed to the fields.
    pub objects: BTreeMap<String, Value>,
    /// Ids asked for that weren't in the world this tick.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gone: Vec<String>,
    /// Reading the world failed this tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Flight rows only: every controller read the game made since the
    /// last tick, as it got them (agent steps merged in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<InputRead>>,
    /// Flight rows only: reads past [`InputRead::MAX_PER_TICK`] not kept.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub input_dropped: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// One `XInputGetState` result as the game got it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InputRead {
    /// The controller port its handle opened on, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u32>,
    pub handle: u32,
    pub status: u32,
    /// The packet number.
    pub packet: u32,
    /// Buttons held (`a`, `rt`, `up`, ...; analog ones when above 0).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<String>,
    pub left: [f64; 2],
    pub right: [f64; 2],
    /// The 18-byte gamepad state as hex, for what the names round off
    /// (analog pressures).
    pub raw: String,
}

impl InputRead {
    /// Reads kept per tick; more (a load, when ticks stop) are counted.
    pub const MAX_PER_TICK: usize = 64;
}

/// `h2 agent flight show`'s filters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShowFilter {
    /// Game ticks from and to, inclusive.
    pub from: Option<u64>,
    pub to: Option<u64>,
    /// Dotted paths into each object (`physics.crouch`, `pos`); `input`
    /// prints the tick's controller reads. Whole rows when empty.
    pub fields: Vec<String>,
}

/// The rows of a trace or flight file that pass `f`, one line each:
/// whole rows as JSON, or with fields `tick N  id path=value ...`.
pub fn show(text: &str, f: &ShowFilter) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: TickRow =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        let tick = row.tick;
        if f.from.is_some_and(|from| tick.is_none_or(|t| t < from))
            || f.to.is_some_and(|to| tick.is_none_or(|t| t > to))
        {
            continue;
        }
        if f.fields.is_empty() {
            out.push(line.trim().to_owned());
            continue;
        }
        let mut s = match tick {
            Some(t) => format!("tick {t}"),
            None => format!("ticks {}", row.ticks),
        };
        for (id, o) in &row.objects {
            s.push_str("  ");
            s.push_str(id);
            for path in f.fields.iter().filter(|p| *p != "input") {
                let v = path
                    .split('.')
                    .try_fold(o, |v, k| v.get(k))
                    .map_or("-".to_owned(), Value::to_string);
                s.push_str(&format!(" {path}={v}"));
            }
        }
        for id in &row.gone {
            s.push_str(&format!("  {id} gone"));
        }
        if let Some(e) = &row.error {
            s.push_str(&format!("  error: {e}"));
        }
        if f.fields.iter().any(|p| p == "input") {
            for r in row.input.iter().flatten() {
                let port = r.port.map_or("?".to_owned(), |p| p.to_string());
                s.push_str(&format!(
                    "  in{port}[{} L{:?} R{:?}]",
                    r.buttons.join("+"),
                    r.left,
                    r.right
                ));
            }
        }
        out.push(s);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rows() -> String {
        (10..15)
            .map(|t| {
                let row = TickRow {
                    tick: Some(t),
                    ticks: t + 100,
                    map: Some("01b_spacestation".into()),
                    objects: [(
                        "player.0".to_owned(),
                        json!({"id": "player.0", "kind": "player",
                               "physics": {"crouch": t as f64 / 10.0}}),
                    )]
                    .into(),
                    gone: if t == 13 {
                        vec!["actor.e0010001".into()]
                    } else {
                        vec![]
                    },
                    input: Some(vec![InputRead {
                        port: Some(0),
                        buttons: vec!["a".into()],
                        left: [0.0, 1.0],
                        ..Default::default()
                    }]),
                    ..Default::default()
                };
                serde_json::to_string(&row).unwrap() + "\n"
            })
            .collect()
    }

    #[test]
    fn show_filters_ticks_and_picks_fields() {
        let all = show(&rows(), &ShowFilter::default()).unwrap();
        assert_eq!(all.len(), 5);
        assert!(all[0].starts_with(r#"{"tick":10,"#), "{}", all[0]);

        let f = ShowFilter {
            from: Some(12),
            to: Some(13),
            fields: vec!["physics.crouch".into(), "pos".into(), "input".into()],
        };
        assert_eq!(
            show(&rows(), &f).unwrap(),
            [
                "tick 12  player.0 physics.crouch=1.2 pos=-  in0[a L[0.0, 1.0] R[0.0, 0.0]]",
                "tick 13  player.0 physics.crouch=1.3 pos=-  actor.e0010001 gone  \
                 in0[a L[0.0, 1.0] R[0.0, 0.0]]",
            ]
        );
    }

    #[test]
    fn show_names_a_bad_line() {
        let e = show(
            "{\"ticks\":1,\"objects\":{}}\nnot json\n",
            &ShowFilter::default(),
        );
        assert!(e.unwrap_err().starts_with("line 2:"));
    }

    #[test]
    fn params_refuse_unknown_fields() {
        let p: TraceParams =
            serde_json::from_value(json!({"objects": ["player.0"], "fields": ["physics"]}))
                .unwrap();
        assert_eq!(p.objects, ["player.0"]);
        assert!(serde_json::from_value::<FlightParams>(json!({"objects": [], "secs": 3})).is_err());
    }
}
