//! The player comes first (docs/runtime/lab.md, "When the player opens a
//! game"). While a job runs, a game the player opens on this Mac asks them,
//! once per game, what the job should do: Stop and pause, Keep running or
//! Finish this one ([`takeover::PlayerGames`] holds the answers). The queue
//! waits while a game that didn't answer Keep running is open.
//!
//! The queue also waits for a game the player allowed a job over but the
//! lab couldn't close ([`takeover::Outcome::Held`]); [`tell`] says so on
//! the Mac.

use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;
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

/// Tells the player on this Mac, as a notification, that the job `name`
/// they allowed is waiting because the lab couldn't close their game, and
/// what to do. `reason` is the gate's, for the host's log. Best effort: a
/// notification that can't be shown leaves the job's note and log.
pub(super) fn tell(name: &str, reason: &str) {
    let (title, body) = notice(name);
    let script = format!(
        "display notification {} with title {}",
        apple_string(&body),
        apple_string(&title)
    );
    match Command::new("/usr/bin/osascript").args(["-e", &script]).output() {
        Ok(out) if out.status.success() => {}
        Ok(out) => eprintln!(
            "lab: telling the player ({reason}): osascript exited with {}",
            out.status
        ),
        Err(e) => eprintln!("lab: telling the player ({reason}): {e}"),
    }
}

/// The notification's title and body for the waiting job `name`.
fn notice(name: &str) -> (String, String) {
    let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
    (
        "Lab job waiting: your game is still open".to_owned(),
        format!(
            "The lab couldn't close your game for \"{name}\". Quit it from its menu or close \
             its window; the job starts once it has closed."
        ),
    )
}

/// `text` as an AppleScript string literal.
fn apple_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_says_what_happened_and_what_to_do() {
        let (title, body) = notice("Menu bands check");
        assert!(title.contains("still open"), "{title}");
        assert!(
            body.contains("couldn't close your game for \"Menu bands check\""),
            "{body}"
        );
        assert!(body.contains("Quit it from its menu or close its window"), "{body}");
        assert!(body.ends_with("the job starts once it has closed."), "{body}");
        let (_, body) = notice("a\nb\u{7}c");
        assert!(body.contains("\"abc\""), "{body}");
    }

    #[test]
    fn apple_strings_escape_quotes_and_backslashes() {
        assert_eq!(apple_string(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
        assert_eq!(apple_string("a\nb"), "\"a b\"");
    }
}
