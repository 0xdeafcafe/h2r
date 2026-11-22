//! Where a conversion's time goes: named phases, timed on the thread that
//! runs them, for the Halo 1 import's timing report
//! (`cli::h1_import`, `campaigns/halo1/import-timing.txt`).
//!
//! [`time`] runs a closure as a phase; [`record`] collects the phases
//! timed inside it on this thread. A phase's time is its own: a phase
//! timed inside another is taken out of the outer one, so the phases of a
//! job add up to the job. Outside a [`record`], [`time`] is the closure and
//! nothing else.

use std::cell::RefCell;
use std::time::Duration;

/// Each phase's own time, in the order first seen: its name, its wall
/// time, and the CPU time its thread spent in it (which a loaded machine
/// does not stretch).
pub type Phases = Vec<(&'static str, Duration, Duration)>;

struct Open {
    name: &'static str,
    start: Duration,
    cpu: Duration,
    /// The wall and CPU time of the phases timed inside this one.
    inner: (Duration, Duration),
}

/// The wall clock the phases are timed on: the process's own. The tests
/// advance theirs by hand ([`tests`]), so a loaded machine cannot stretch
/// what they measure.
fn now() -> Duration {
    #[cfg(not(test))]
    {
        static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        BASE.get_or_init(std::time::Instant::now).elapsed()
    }
    #[cfg(test)]
    {
        TEST_NOW.with(std::cell::Cell::get)
    }
}

#[cfg(test)]
thread_local! {
    /// The tests' wall clock, advanced by hand.
    static TEST_NOW: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
}

/// What tells this thread's CPU time: set once by the program
/// ([`set_cpu_clock`]; this crate has no `unsafe` to ask the system
/// itself). Unset, every CPU time is zero.
static CPU_CLOCK: std::sync::OnceLock<fn() -> Duration> = std::sync::OnceLock::new();

/// Sets what tells the calling thread's CPU time (`CLOCK_THREAD_CPUTIME_ID`).
pub fn set_cpu_clock(clock: fn() -> Duration) {
    let _ = CPU_CLOCK.set(clock);
}

fn thread_cpu() -> Duration {
    CPU_CLOCK.get().map_or(Duration::ZERO, |f| f())
}

#[derive(Default)]
struct Recorder {
    open: Vec<Open>,
    done: Phases,
    notes: Vec<String>,
}

/// What a [`record`] saw.
#[derive(Clone, Debug, Default)]
pub struct Record {
    /// Each phase's own time; the rest is `other`.
    pub phases: Phases,
    /// What [`note`] was told, in order.
    pub notes: Vec<String>,
}

/// A line for the report about the job being recorded (a retry, say);
/// nothing outside a [`record`].
pub fn note(text: impl FnOnce() -> String) {
    REC.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            rec.notes.push(text());
        }
    });
}

thread_local! {
    static REC: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

/// Runs `f` as the phase `name`.
pub fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    let _phase = start(name);
    f()
}

/// The phase `name` from here until the guard is dropped (the end of the
/// scope, a panic included). Guards close in the reverse of their opening,
/// as scopes do.
#[must_use = "the phase ends when the guard is dropped"]
pub fn start(name: &'static str) -> Phase {
    let open = REC.with(|r| {
        let mut r = r.borrow_mut();
        let Some(rec) = r.as_mut() else { return false };
        rec.open.push(Open {
            name,
            start: now(),
            cpu: thread_cpu(),
            inner: (Duration::ZERO, Duration::ZERO),
        });
        true
    });
    Phase { open }
}

/// An open phase ([`start`]).
pub struct Phase {
    open: bool,
}

impl Drop for Phase {
    fn drop(&mut self) {
        if !self.open {
            return;
        }
        REC.with(|r| {
            let mut r = r.borrow_mut();
            let Some(rec) = r.as_mut() else { return };
            let Some(o) = rec.open.pop() else { return };
            let all = (now().saturating_sub(o.start), thread_cpu().saturating_sub(o.cpu));
            if let Some(parent) = rec.open.last_mut() {
                parent.inner.0 += all.0;
                parent.inner.1 += all.1;
            }
            let own = (
                all.0.saturating_sub(o.inner.0),
                all.1.saturating_sub(o.inner.1),
            );
            add(&mut rec.done, o.name, own);
        });
    }
}

fn add(phases: &mut Phases, name: &'static str, (wall, cpu): (Duration, Duration)) {
    match phases.iter_mut().find(|p| p.0 == name) {
        Some(p) => {
            p.1 += wall;
            p.2 += cpu;
        }
        None => phases.push((name, wall, cpu)),
    }
}

/// Runs `f` and returns the phases timed inside it on this thread, and
/// the rest of its time as `other`, with its notes.
pub fn record<T>(f: impl FnOnce() -> T) -> (T, Record) {
    // The recorder this one replaces, put back on the way out (a panic
    // included).
    struct Restore(Option<Recorder>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let outer = self.0.take();
            REC.with(|r| *r.borrow_mut() = outer);
        }
    }
    let _restore = Restore(REC.with(|r| r.borrow_mut().replace(Recorder::default())));
    let v = time("other", f);
    let rec = REC.with(|r| r.borrow_mut().take()).unwrap_or_default();
    (
        v,
        Record {
            phases: rec.done,
            notes: rec.notes,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Advances the test clock: what a sleep would be, without one.
    fn spin(ms: u64) {
        TEST_NOW.with(|n| n.set(n.get() + Duration::from_millis(ms)));
    }

    #[test]
    fn phases_are_their_own_time_and_add_up_to_the_whole() {
        let start = now();
        let (v, rec) = record(|| {
            spin(5);
            time("a", || {
                spin(20);
                time("b", || spin(30));
            });
            time("b", || spin(10));
            note(|| "retried".into());
            7
        });
        let whole = now() - start;
        assert_eq!(v, 7);
        assert_eq!(rec.notes, ["retried"]);
        let phases = rec.phases;
        let names: Vec<_> = phases.iter().map(|p| p.0).collect();
        assert_eq!(names, ["b", "a", "other"]);
        let get = |n| phases.iter().find(|p| p.0 == n).unwrap().1;
        assert_eq!(get("b"), Duration::from_millis(40));
        // "a" is the 20 it slept itself: the 30 of the "b" inside it is
        // "b"'s, not "a"'s.
        assert_eq!(get("a"), Duration::from_millis(20));
        let sum: Duration = phases.iter().map(|p| p.1).sum();
        assert_eq!(sum, whole);
        // With no clock set, no CPU time.
        assert!(phases.iter().all(|p| p.2.is_zero()), "{phases:?}");
    }

    #[test]
    fn outside_a_record_a_phase_is_just_the_closure() {
        assert_eq!(time("x", || 3), 3);
        note(|| unreachable!("not recording"));
        let (_, r) = record(|| ());
        assert_eq!(r.phases.iter().map(|p| p.0).collect::<Vec<_>>(), ["other"]);
    }
}
