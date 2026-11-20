use std::time::Duration;

use agent_proto::host_lobby::Phase;
use live_proto::hosts::EndReason;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Wait,
    Start,
    Playing(u8),
    End(EndReason),
}

pub(super) enum Observation {
    Lobby {
        phase: Phase,
        players: u8,
        ready: bool,
    },
    ChildExited,
    Stop,
}

pub(super) struct Lifecycle {
    idle_timeout: Duration,
    max_duration: Duration,
    start_players: usize,
    start_requested: bool,
    playing_reported: bool,
    pub players_peak: u8,
}

impl Lifecycle {
    pub fn new(idle_timeout_s: u32, max_match_s: u32, roster_size: usize) -> Self {
        Self {
            idle_timeout: Duration::from_secs(u64::from(idle_timeout_s)),
            max_duration: Duration::from_secs(u64::from(max_match_s)),
            start_players: roster_size.max(1),
            start_requested: false,
            playing_reported: false,
            players_peak: 0,
        }
    }

    pub fn observe(
        &mut self,
        joinable_for: Duration,
        lease_age: Duration,
        observation: Observation,
    ) -> Action {
        let (phase, players, ready) = match observation {
            Observation::Lobby {
                phase,
                players,
                ready,
            } => (phase, players, ready),
            Observation::ChildExited => return Action::End(EndReason::Crashed),
            Observation::Stop => return Action::End(EndReason::Stopped),
        };
        self.players_peak = self.players_peak.max(players);

        if phase == Phase::Postgame {
            return Action::End(EndReason::MatchOver);
        }
        if players == 0 && self.players_peak > 0 {
            return Action::End(EndReason::Empty);
        }
        if lease_age >= self.max_duration {
            return Action::End(EndReason::MaxDuration);
        }
        if players == 0 && joinable_for >= self.idle_timeout {
            return Action::End(EndReason::Idle);
        }

        if phase == Phase::Playing && !self.playing_reported {
            self.playing_reported = true;
            return Action::Playing(players);
        }
        if phase == Phase::Lobby
            && ready
            && usize::from(players) >= self.start_players
            && !self.start_requested
        {
            self.start_requested = true;
            return Action::Start;
        }
        Action::Wait
    }
}

pub(super) fn exit_code(reason: &EndReason) -> u8 {
    match reason {
        EndReason::BootTimeout | EndReason::Crashed => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
