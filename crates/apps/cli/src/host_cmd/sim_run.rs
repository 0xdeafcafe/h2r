use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Result, ensure};
use live_proto::hosts::{EndReason, HostLease};
use serde_json::Value;

use super::live::Live;
use super::process::{Game, stopping};

const POLL: Duration = Duration::from_millis(200);
const AGENT_TIMEOUT: Duration = Duration::from_millis(500);

/// A bot-filled player lobby uses the regular lobby API.
pub(super) fn assigned(assignment_id: &str, bots: &[String]) -> bool {
    !bots.is_empty()
        && assignment_id
            .strip_prefix("sim-")
            .is_some_and(|id| !id.is_empty())
}

pub(super) fn configure(
    cmd: &mut Command,
    lease: &HostLease,
    disc: &Path,
    out: &Path,
    data: &Path,
    socket: &Path,
) -> Result<()> {
    cmd.args(["sim", "host-run"])
        .arg(disc)
        .arg(out)
        .arg(data)
        .env("H2_HOST_LEASE_JSON", serde_json::to_string(lease)?)
        .env(agent_proto::ENV, socket)
        .env_remove("H2_HOST_LOBBY")
        .env_remove("H2_TUNNEL_LISTEN");
    Ok(())
}

pub(super) fn wait(
    game: &mut Game,
    live: &Live<'_>,
    players: u8,
    players_peak: &mut u8,
) -> Result<EndReason> {
    loop {
        if stopping() {
            return Ok(EndReason::Stopped);
        }
        let exited = game.exited()?;
        if let Some(status) = &exited {
            ensure!(status.success(), "sim game child exited with {status}");
        }
        // A short unpaced sim can finish between polls. It still reports
        // Playing before Ended when the child has uploaded its report.
        if *players_peak == 0 && (exited.is_some() || match_started(game)) {
            *players_peak = players;
            live.playing(players)?;
        }
        if exited.is_some() {
            return Ok(EndReason::MatchOver);
        }
        thread::sleep(POLL);
    }
}

fn match_started(game: &Game) -> bool {
    let state = game.connect(AGENT_TIMEOUT).ok().and_then(|mut agent| {
        agent.call(agent_proto::method::MATCH_STATUS, Value::Null).ok()
    });
    state.as_ref().is_some_and(played_tick)
}

fn played_tick(state: &Value) -> bool {
    matches!(state["phase"].as_str(), Some("playing" | "over"))
        && state["ticks"].as_u64().is_some_and(|ticks| ticks > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_sim_assignments_with_bot_seats_use_sim_boot() {
        let bots = ["Clanker".to_string()];
        assert!(assigned("sim-run_42", &bots));
        assert!(!assigned("sim-run_42", &[]));
        assert!(!assigned("sim-", &bots));
        assert!(!assigned("custom-run_42", &bots));
    }

    #[test]
    fn sim_start_requires_a_played_tick() {
        assert!(!played_tick(&json!({"phase": "waiting", "ticks": 0})));
        assert!(!played_tick(&json!({"phase": "binding", "ticks": 0})));
        assert!(!played_tick(&json!({"phase": "playing", "ticks": 0})));
        assert!(played_tick(&json!({"phase": "playing", "ticks": 1})));
        assert!(played_tick(&json!({"phase": "over", "ticks": 2})));
    }
}
