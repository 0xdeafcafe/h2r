use super::*;

fn lobby(phase: Phase, players: u8, ready: bool) -> Observation {
    Observation::Lobby {
        phase,
        players,
        ready,
    }
}

fn at(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn observe(host: &mut Lifecycle, seconds: u64, sample: Observation) -> Action {
    host.observe(at(seconds), at(seconds), sample)
}

#[test]
fn starts_once_when_the_reserved_number_of_players_is_ready() {
    let mut host = Lifecycle::new(120, 1800, 2);
    assert_eq!(
        observe(&mut host, 0, lobby(Phase::Lobby, 1, true)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 1, lobby(Phase::Lobby, 2, false)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 2, lobby(Phase::Lobby, 2, true)),
        Action::Start
    );
    assert_eq!(
        observe(&mut host, 3, lobby(Phase::Lobby, 2, true)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 4, lobby(Phase::Loading, 2, true)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 5, lobby(Phase::Playing, 2, true)),
        Action::Playing(2)
    );
    assert_eq!(
        observe(&mut host, 6, lobby(Phase::Playing, 2, true)),
        Action::Wait
    );
}

#[test]
fn open_lobby_needs_a_player_even_if_the_agent_reports_ready() {
    let mut host = Lifecycle::new(120, 1800, 0);
    assert_eq!(
        observe(&mut host, 0, lobby(Phase::Lobby, 0, true)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 1, lobby(Phase::Lobby, 1, true)),
        Action::Start
    );
}

#[test]
fn postgame_wins_when_the_players_already_left_or_the_deadline_arrived() {
    let mut host = Lifecycle::new(120, 1800, 2);
    assert_eq!(
        observe(&mut host, 2, lobby(Phase::Playing, 2, true)),
        Action::Playing(2)
    );
    assert_eq!(
        observe(&mut host, 3, lobby(Phase::Playing, 3, true)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 1800, lobby(Phase::Postgame, 0, false)),
        Action::End(EndReason::MatchOver)
    );
    assert_eq!(host.players_peak, 3);
}

#[test]
fn last_player_leaving_ends_the_host_even_before_playing() {
    let mut host = Lifecycle::new(120, 1800, 2);
    assert_eq!(
        observe(&mut host, 2, lobby(Phase::Lobby, 1, true)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 3, lobby(Phase::Lobby, 0, false)),
        Action::End(EndReason::Empty)
    );
    assert_eq!(host.players_peak, 1);
}

#[test]
fn unvisited_lobby_expires_at_its_idle_deadline() {
    let mut host = Lifecycle::new(120, 1800, 0);
    assert_eq!(
        observe(&mut host, 119, lobby(Phase::Lobby, 0, false)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 120, lobby(Phase::Lobby, 0, false)),
        Action::End(EndReason::Idle)
    );
}

#[test]
fn occupied_host_ignores_idle_deadline_and_ends_at_hard_limit() {
    let mut host = Lifecycle::new(120, 1800, 0);
    assert_eq!(
        observe(&mut host, 120, lobby(Phase::Playing, 1, true)),
        Action::Playing(1)
    );
    assert_eq!(
        observe(&mut host, 1799, lobby(Phase::Playing, 1, true)),
        Action::Wait
    );
    assert_eq!(
        observe(&mut host, 1800, lobby(Phase::Playing, 1, true)),
        Action::End(EndReason::MaxDuration)
    );
}

#[test]
fn idle_uses_joinable_time_and_hard_limit_includes_boot() {
    let mut host = Lifecycle::new(120, 1800, 0);
    assert_eq!(
        host.observe(at(110), at(140), lobby(Phase::Lobby, 0, false)),
        Action::Wait
    );
    assert_eq!(
        host.observe(at(1750), at(1800), lobby(Phase::Playing, 1, true)),
        Action::End(EndReason::MaxDuration)
    );
}

#[test]
fn child_exit_reports_crashed_even_with_players_present() {
    let mut host = Lifecycle::new(120, 1800, 0);
    assert_eq!(
        observe(&mut host, 1, lobby(Phase::Playing, 1, true)),
        Action::Playing(1)
    );
    assert_eq!(
        observe(&mut host, 2, Observation::ChildExited),
        Action::End(EndReason::Crashed)
    );
    assert_eq!(host.players_peak, 1);
}

#[test]
fn provider_stop_is_a_clean_end() {
    let mut host = Lifecycle::new(120, 1800, 0);
    assert_eq!(
        observe(&mut host, 0, Observation::Stop),
        Action::End(EndReason::Stopped)
    );
}

#[test]
fn boot_timeout_and_crash_exit_nonzero() {
    for reason in [EndReason::Crashed, EndReason::BootTimeout] {
        assert_eq!(exit_code(&reason), 1, "{reason:?}");
    }
    for reason in [
        EndReason::MatchOver,
        EndReason::Empty,
        EndReason::Idle,
        EndReason::MaxDuration,
        EndReason::Stopped,
    ] {
        assert_eq!(exit_code(&reason), 0, "{reason:?}");
    }
}
