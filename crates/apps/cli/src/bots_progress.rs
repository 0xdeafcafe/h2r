//! `progress.json`: what `h2 bots test` knows right now, in one small file
//! an outside driver (`--no-drive`, an LLM or a person) can poll: the
//! checks as pass, pending or fail, the phase, mode and map the agent API
//! last reported, the game's last `ui:` line and its last `bots:` or
//! `clankers:` line. It is written from the merged log, so it says what
//! `h2 bots verdict` would.

use std::fs;
use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use crate::bots_verdict::{self, Verdict};
use crate::coop_verdict::parse_line;

/// The file's name in the run's log directory.
pub const FILE: &str = "progress.json";

/// The progress of `log` (the merged log's text) as pretty JSON, or `None`
/// before the plan line. `finished`: the run is over, so a check not
/// passed is `fail`, not `pending`.
pub fn render(log: &str, finished: bool) -> Option<String> {
    let verdict = bots_verdict::verdict(log)?;
    Some(serde_json::to_string_pretty(&value(log, &verdict, finished)).expect("json"))
}

fn value(log: &str, verdict: &Verdict, finished: bool) -> Value {
    let (mut phase, mut mode, mut map) = ("-", "-", "-");
    let (mut ui, mut bots) = ("", "");
    for (_, text) in log.lines().filter_map(parse_line) {
        if let Some(s) = bots_verdict::read_status(text) {
            (phase, mode, map) = s;
        } else if text.starts_with("ui:") {
            ui = text;
        } else if text.starts_with("bots:") || text.starts_with("clankers:") {
            bots = text;
        }
    }
    let checks: Vec<Value> = verdict
        .checks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let state = match (c.pass, finished) {
                (true, _) => "pass",
                (false, true) => "fail",
                (false, false) => "pending",
            };
            json!({ "step": i + 1, "name": c.name, "state": state, "detail": c.detail })
        })
        .collect();
    let next = verdict
        .first_failure()
        .map(|(n, c)| json!({ "step": n, "name": c.name }));
    json!({
        "finished": finished,
        "passed": verdict.passed(),
        "next": next,
        "phase": phase,
        "mode": mode,
        "map": map,
        "last_ui": ui,
        "last_bots": bots,
        "checks": checks,
    })
}

/// Writes `text` to `dir`/progress.json through a temporary file, so a
/// poller never reads half of it.
pub fn write(dir: &Path, text: &str) -> Result<()> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, dir.join(FILE))?;
    Ok(())
}

#[cfg(test)]
mod tests;
