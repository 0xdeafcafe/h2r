//! Background work in the game process (docs/runtime/background-work.md).
//!
//! Work nobody is waiting on this frame (bot nav meshes and visibility
//! grids, hashing a map for a film, indexing sounds, uploads) runs on
//! threads below the game's own: utility QoS on Apple systems, nice
//! [`NICE`] on Linux. The scheduler then gives the guest, render, audio
//! and input threads the cores first, and a map load never waits behind a
//! background build for CPU.
//!
//! Every thread a background job starts lowers itself ([`spawn`],
//! [`spawn_scoped`], or [`lower_current`] first thing): a QoS class set on
//! one thread does not reliably carry to the threads it spawns.
//!
//! A job that splits across threads takes [`workers`] of them: a quarter
//! of the cores, at least one, or [`ENV`].

use std::io;
use std::thread::{Builder, JoinHandle, Scope, ScopedJoinHandle};

/// `H2_BACKGROUND_THREADS`: how many threads one background job may split
/// across (a positive number). Unset, `0` or not a number: a quarter of
/// the cores, at least one ([`workers_from`]).
pub const ENV: &str = "H2_BACKGROUND_THREADS";

/// The nice value a background thread takes on Linux.
pub const NICE: i32 = 10;

/// The worker cap for a background job: [`ENV`]'s value when it is a
/// positive number (at most `cpus`, at least one), else a quarter of
/// `cpus`, at least one.
pub fn workers_from(value: Option<&str>, cpus: usize) -> usize {
    let cpus = cpus.max(1);
    match value.and_then(|v| v.trim().parse::<usize>().ok()) {
        Some(n) if n > 0 => n.min(cpus),
        _ => (cpus / 4).max(1),
    }
}

/// The worker cap for a background job on this machine ([`workers_from`]
/// over [`ENV`] and the available cores).
pub fn workers() -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    workers_from(std::env::var(ENV).ok().as_deref(), cpus)
}

/// Spawns `f` on a thread named `name` that lowers itself
/// ([`lower_current`]) before it runs `f`.
pub fn spawn<F, T>(name: impl Into<String>, f: F) -> io::Result<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().name(name.into()).spawn(move || {
        lower_current();
        f()
    })
}

/// [`spawn`] inside a [`std::thread::scope`].
pub fn spawn_scoped<'scope, 'env, F, T>(
    scope: &'scope Scope<'scope, 'env>,
    name: impl Into<String>,
    f: F,
) -> io::Result<ScopedJoinHandle<'scope, T>>
where
    F: FnOnce() -> T + Send + 'scope,
    T: Send + 'scope,
{
    Builder::new().name(name.into()).spawn_scoped(scope, move || {
        lower_current();
        f()
    })
}

/// Lowers the calling thread to background priority: `QOS_CLASS_UTILITY`
/// on Apple systems (as macOS's own indexing runs), nice [`NICE`] on Linux
/// (a nice value there is the thread's own). Elsewhere it does nothing.
/// Failure is ignored: the thread then runs at the default.
pub fn lower_current() {
    imp::lower();
}

#[cfg(target_vendor = "apple")]
mod imp {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    }
    /// `QOS_CLASS_UTILITY` from `sys/qos.h`.
    const QOS_CLASS_UTILITY: u32 = 0x11;

    pub fn lower() {
        // SAFETY: sets the calling thread's own QoS class; no pointers.
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    pub fn lower() {
        // SAFETY: plain priority change; on Linux `who = 0` is the calling
        // thread, not the whole process.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, super::NICE);
        }
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
mod imp {
    pub fn lower() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_a_quarter_of_the_cores_and_at_least_one() {
        assert_eq!(workers_from(None, 16), 4);
        assert_eq!(workers_from(None, 10), 2);
        assert_eq!(workers_from(None, 3), 1);
        assert_eq!(workers_from(None, 1), 1);
        assert_eq!(workers_from(None, 0), 1);
    }

    #[test]
    fn the_switch_sets_the_cap_within_the_cores() {
        assert_eq!(workers_from(Some("3"), 16), 3);
        assert_eq!(workers_from(Some(" 6 "), 16), 6);
        assert_eq!(workers_from(Some("64"), 8), 8);
        assert_eq!(workers_from(Some("1"), 1), 1);
    }

    #[test]
    fn a_zero_or_bad_value_keeps_the_default() {
        assert_eq!(workers_from(Some("0"), 16), 4);
        assert_eq!(workers_from(Some(""), 16), 4);
        assert_eq!(workers_from(Some("all"), 16), 4);
        assert_eq!(workers_from(Some("-2"), 16), 4);
    }

    #[test]
    fn a_spawned_thread_runs_named() {
        let name = spawn("bg-test", || std::thread::current().name().map(str::to_owned))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(name.as_deref(), Some("bg-test"));
        let sum: u32 = std::thread::scope(|s| {
            let hs: Vec<_> = (0..3u32)
                .map(|k| spawn_scoped(s, format!("bg-scoped-{k}"), move || k).unwrap())
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).sum()
        });
        assert_eq!(sum, 3);
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn a_spawned_thread_runs_at_utility_qos() {
        unsafe extern "C" {
            fn qos_class_self() -> u32;
        }
        // SAFETY: reads the calling thread's own class.
        let class = spawn("bg-qos", || unsafe { qos_class_self() }).unwrap().join().unwrap();
        assert_eq!(class, 0x11);
    }
}
