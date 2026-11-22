//! The player comes first (docs/runtime/lab.md, "When the player opens a
//! game"). While a job runs, a game the player opens on this Mac asks them,
//! once per game, what the job should do: Stop and pause, Keep running or
//! Finish this one ([`takeover::PlayerGames`] holds the answers). The queue
//! waits while a game that didn't answer Keep running is open.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{Duration, Instant};

use agent_proto::takeover::Request as TakeoverRequest;

use super::super::takeover::{self, Choice};
use super::Shared;

/// How often a running job, or a held queue, looks for the player's games.
pub(super) const INTERVAL: Duration = Duration::from_secs(4);

/// A job's look for the player's games, from the job's monitor tick.
pub(super) struct Watch {
    /// The job as the start consent described it; asked as `Ask::Running`.
    request: TakeoverRequest,
    next: Instant,
}

impl Watch {
    pub(super) fn new(request: TakeoverRequest) -> Self {
        Self {
            request,
            next: Instant::now(),
        }
    }

    /// One tick of the job's monitor: at most every [`INTERVAL`] while the
    /// job builds or runs, look for the player's games, ask new ones and
    /// record what they answer. True when the player chose Stop and pause
    /// and the job is stopping.
    pub(super) fn tick(&mut self, sh: &Shared, id: &str) -> bool {
        let now = Instant::now();
        if now < self.next || !working(sh, id) {
            return false;
        }
        self.next = now + INTERVAL;
        let seen = match sh
            .takeover
            .player_games(&|socket, pid| sh.owns_game(socket, pid))
        {
            Ok(seen) => seen,
            Err(e) => {
                eprintln!("lab: job {id}: looking for the player's games: {e:#}");
                return false;
            }
        };
        let request = &self.request;
        let settled = sh.player.lock().unwrap().observe(
            &seen,
            now,
            sh.takeover.answer_timeout(),
            |path| sh.takeover.ask_running(path, request.clone()),
            takeover::poll,
        );
        let mut pause = false;
        for s in settled {
            let note = s.note();
            eprintln!("lab: job {id}: {note}");
            log(sh, id, &note);
            sh.update(id, |m| m.player.push(note));
            pause |= s.choice == Choice::StopAndPause;
        }
        pause && pause_job(sh, id)
    }
}

/// Whether the queue waits for the player: a game of theirs is still open
/// that didn't answer Keep running. Looks at the Mac only while one was.
pub(super) fn holds(sh: &Shared) -> bool {
    if !sh.player.lock().unwrap().holds() {
        return false;
    }
    let seen = match sh
        .takeover
        .player_games(&|socket, pid| sh.owns_game(socket, pid))
    {
        Ok(seen) => seen,
        Err(e) => {
            // Wait rather than start a job over the player's game.
            eprintln!("lab: looking for the player's games: {e:#}");
            return true;
        }
    };
    let mut games = sh.player.lock().unwrap();
    games.refresh(
        &seen,
        Instant::now(),
        sh.takeover.answer_timeout(),
        takeover::poll,
    );
    games.holds()
}

/// The job is past its start consent and taking the Mac: running, or
/// building an image that isn't cached.
fn working(sh: &Shared, id: &str) -> bool {
    let inner = sh.inner.lock().unwrap();
    inner.active.as_ref().is_some_and(|a| a.id == id && !a.stop)
        && inner.jobs.get(id).is_some_and(|m| {
            m.state == "running" || (m.state == "building" && m.cached == Some(false))
        })
}

/// Adds `line` to the job's run log, or its build log before the run.
fn log(sh: &Shared, id: &str, line: &str) {
    let dir = sh.cfg.dir("runs").join(id);
    let path = [dir.join("run.log"), dir.join("build.log")]
        .into_iter()
        .find(|p| p.is_file());
    if let Some(path) = path
        && let Ok(mut file) = OpenOptions::new().append(true).open(path)
    {
        let _ = writeln!(file, "lab: {line}");
    }
}

/// Stops job `id` as `h2 lab stop` does (SIGTERM to its process group,
/// SIGKILL after the grace) and marks it to go back to the front of the
/// queue. False when it was already stopping.
fn pause_job(sh: &Shared, id: &str) -> bool {
    let pgid = {
        let mut inner = sh.inner.lock().unwrap();
        let Some(a) = inner.active.as_mut().filter(|a| a.id == id && !a.stop) else {
            return false;
        };
        a.stop = true;
        a.pause = true;
        a.pgid
    };
    let Some(pgid) = pgid else {
        // Between steps: the next one won't start.
        return true;
    };
    // SAFETY: the job's own process group.
    unsafe { libc::kill(-pgid, libc::SIGTERM) };
    let still_running = || {
        sh.inner
            .lock()
            .unwrap()
            .active
            .as_ref()
            .is_some_and(|a| a.id == id && a.pgid == Some(pgid))
    };
    let deadline = Instant::now() + sh.cfg.grace;
    while Instant::now() < deadline && still_running() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let inner = sh.inner.lock().unwrap();
    if inner
        .active
        .as_ref()
        .is_some_and(|a| a.id == id && a.pgid == Some(pgid))
    {
        // SAFETY: still the job's group.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    true
}
