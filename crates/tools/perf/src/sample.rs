//! The sampler: a host thread that, at a fixed rate, suspends each target
//! thread with Mach's `thread_suspend`, reads its registers with
//! `thread_get_state`, walks its frame-pointer chain, and resumes it.
//!
//! On Windows (x86-64; the Xbox app, docs/runtime/xbox.md) it is
//! `SuspendThread`, `GetThreadContext` and a copy of the live stack, then,
//! with the thread running again, `RtlVirtualUnwind` over the copy (see
//! `imp` there): Windows x64 code keeps no frame-pointer chain, and the
//! unwinder's table lookup may take locks the suspended thread holds.
//!
//! Why this and not a signal or `sample(1)`: a `SIGPROF` handler runs on
//! the target thread, which may be inside generated code holding the guest
//! memory lock or inside malloc; `thread_get_state` needs no cooperation
//! and no async-signal-safety argument. `sample(1)` and xctrace need root
//! or a separate process and give no hook to map generated code back to
//! guest addresses while the dylib is still loaded. Frame pointers are
//! reliable here: the Apple arm64 and x86_64 ABIs keep them, Rust's Apple
//! targets keep them, and the emitter's clang flags do not omit them.
//!
//! While a target is suspended the sampler only reads registers and stack
//! words into a fixed buffer: no allocation, no locks (the target may hold
//! the allocator's or the runtime's). Everything else happens after
//! `thread_resume`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Deepest stack kept per sample.
pub const MAX_FRAMES: usize = 192;

/// A thread to sample. Make it on that thread with [`ThreadTarget::current`].
#[derive(Debug, Clone)]
pub struct ThreadTarget {
    pub name: String,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    port: u32,
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    stack: (u64, u64),
    /// A real handle to the thread (`DuplicateHandle` of the pseudo
    /// handle), never closed: targets live as long as the process.
    #[cfg(windows)]
    handle: isize,
    /// The thread's cycle count at the last sample: grown means it ran.
    #[cfg(windows)]
    cycles: Arc<std::sync::atomic::AtomicU64>,
}

/// One stack: `frames[0]` is the interrupted PC, then return addresses
/// from the frame chain, innermost first. `lr` is the arm64 link register
/// (0 elsewhere): when the leaf has not pushed a frame yet it holds the
/// leaf's return address, which the chain skips.
#[derive(Debug, Clone)]
pub struct RawSample {
    pub thread: usize,
    /// Since the sampler started.
    pub at: Duration,
    /// The thread was running (not blocked or waiting) when sampled.
    pub on_cpu: bool,
    pub frames: Vec<u64>,
    pub lr: u64,
}

/// What a sampling run collected.
#[derive(Debug, Clone, Default)]
pub struct RawProfile {
    pub threads: Vec<String>,
    pub samples: Vec<RawSample>,
    pub wall: Duration,
    pub hz: u32,
    /// Suspends that failed (the thread had exited) or stacks that could
    /// not be read.
    pub missed: u64,
    /// Mean time each target was held suspended, in nanoseconds.
    pub mean_pause_ns: u64,
}

/// A running sampler. [`Sampler::finish`] stops it and returns the samples.
pub struct Sampler {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<RawProfile>>,
}

/// Is sampling supported on this host?
pub const SUPPORTED: bool =
    cfg!(any(target_os = "macos", all(windows, target_arch = "x86_64")));

impl Sampler {
    /// Sample `targets` at `hz` (clamped to 1..=10000) until [`finish`].
    ///
    /// [`finish`]: Sampler::finish
    pub fn start(targets: Vec<ThreadTarget>, hz: u32) -> std::io::Result<Sampler> {
        if !SUPPORTED {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "the sampling profiler runs on macOS and x86-64 Windows only",
            ));
        }
        if targets.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no threads to sample",
            ));
        }
        let hz = hz.clamp(1, 10_000);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("h2-perf-sampler".into())
            .spawn(move || run(targets, hz, &flag))?;
        Ok(Sampler {
            stop,
            handle: Some(handle),
        })
    }

    /// Stop sampling and return what was collected.
    pub fn finish(mut self) -> RawProfile {
        self.stop.store(true, Ordering::Release);
        self.handle
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(targets: Vec<ThreadTarget>, hz: u32, stop: &AtomicBool) -> RawProfile {
    let period = Duration::from_nanos(1_000_000_000 / hz as u64);
    let start = Instant::now();
    let mut out = RawProfile {
        threads: targets.iter().map(|t| t.name.clone()).collect(),
        hz,
        ..RawProfile::default()
    };
    let mut buf = [0u64; MAX_FRAMES];
    let mut pause_total = 0u128;
    let mut pauses = 0u64;
    let mut next = start + period;
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        }
        // Don't try to catch up after a long stall: that would burst.
        next = Instant::now().max(next) + period;
        for (i, t) in targets.iter().enumerate() {
            let at = start.elapsed();
            let t0 = Instant::now();
            match imp::capture(t, &mut buf) {
                Some(c) => {
                    pause_total += t0.elapsed().as_nanos();
                    pauses += 1;
                    out.samples.push(RawSample {
                        thread: i,
                        at,
                        on_cpu: c.on_cpu,
                        frames: buf[..c.depth].to_vec(),
                        lr: c.lr,
                    });
                }
                None => out.missed += 1,
            }
        }
    }
    out.wall = start.elapsed();
    out.mean_pause_ns = pause_total.checked_div(pauses as u128).unwrap_or(0) as u64;
    out
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
struct Captured {
    depth: usize,
    lr: u64,
    on_cpu: bool,
}

/// Strip pointer-authentication and tag bits from a code address. User
/// addresses on macOS are below 2^47.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn strip(p: u64) -> u64 {
    p & 0x0000_7fff_ffff_ffff
}

/// Walk the frame chain from `fp`: each record is `[fp] = caller's fp`,
/// `[fp + 8] = return address` on both arm64 and x86_64. Every record must
/// lie inside `stack`, be aligned, and move strictly towards the stack
/// base, so a corrupt chain ends the walk instead of a wild read.
///
/// # Safety
/// `[stack.0, stack.1)` must be readable memory for the whole call (the
/// target thread's stack, with the thread suspended).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
unsafe fn walk(mut fp: u64, stack: (u64, u64), buf: &mut [u64], mut depth: usize) -> usize {
    while depth < buf.len() {
        if fp < stack.0 || fp.saturating_add(16) > stack.1 || !fp.is_multiple_of(8) {
            break;
        }
        // SAFETY: inside the readable stack range, checked above.
        let (next, ret) = unsafe {
            let p = fp as *const u64;
            (
                std::ptr::read_volatile(p),
                std::ptr::read_volatile(p.add(1)),
            )
        };
        let ret = strip(ret);
        if ret == 0 {
            break;
        }
        buf[depth] = ret;
        depth += 1;
        if next <= fp {
            break;
        }
        fp = next;
    }
    depth
}

/// Points every word of `words` that lies in the live stack range
/// `[range.0, range.1)` at the same place in a copy of it starting at
/// `to`: saved frame pointers and stack addresses in registers, so an
/// unwinder walking the copy never reads the live stack.
#[cfg_attr(not(windows), allow(dead_code))]
fn relocate(words: &mut [u64], range: (u64, u64), to: u64) {
    for w in words {
        if (range.0..range.1).contains(w) {
            *w = *w - range.0 + to;
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    #![allow(unsafe_code)]
    use super::{Captured, ThreadTarget, strip, walk};

    type KernReturn = i32;
    type ThreadAct = u32;

    unsafe extern "C" {
        fn thread_suspend(thread: ThreadAct) -> KernReturn;
        fn thread_resume(thread: ThreadAct) -> KernReturn;
        fn thread_get_state(
            thread: ThreadAct,
            flavor: i32,
            state: *mut u32,
            count: *mut u32,
        ) -> KernReturn;
        fn thread_info(
            thread: ThreadAct,
            flavor: u32,
            info: *mut i32,
            count: *mut u32,
        ) -> KernReturn;
    }

    const THREAD_BASIC_INFO: u32 = 3;
    const THREAD_BASIC_INFO_COUNT: u32 = 10;
    const TH_STATE_RUNNING: i32 = 1;

    #[cfg(target_arch = "aarch64")]
    mod regs {
        /// ARM_THREAD_STATE64: x0..x28, fp, lr, sp, pc (u64), cpsr, flags.
        pub const FLAVOR: i32 = 6;
        pub const COUNT: u32 = 68;
        pub const FP: usize = 29;
        pub const LR: usize = 30;
        pub const SP: usize = 31;
        pub const PC: usize = 32;
    }
    #[cfg(target_arch = "x86_64")]
    mod regs {
        /// x86_THREAD_STATE64: rax rbx rcx rdx rdi rsi rbp rsp r8..r15 rip ...
        pub const FLAVOR: i32 = 4;
        pub const COUNT: u32 = 42;
        pub const FP: usize = 6;
        pub const LR: usize = usize::MAX;
        pub const SP: usize = 7;
        pub const PC: usize = 16;
    }

    impl ThreadTarget {
        /// The calling thread, named `name` in the profile.
        pub fn current(name: impl Into<String>) -> ThreadTarget {
            // SAFETY: plain queries about the calling thread.
            let (port, hi, size) = unsafe {
                let me = libc::pthread_self();
                (
                    libc::pthread_mach_thread_np(me),
                    libc::pthread_get_stackaddr_np(me) as u64,
                    libc::pthread_get_stacksize_np(me) as u64,
                )
            };
            ThreadTarget {
                name: name.into(),
                port,
                stack: (hi.saturating_sub(size), hi),
            }
        }
    }

    fn on_cpu(port: u32) -> bool {
        let mut info = [0i32; THREAD_BASIC_INFO_COUNT as usize];
        let mut count = THREAD_BASIC_INFO_COUNT;
        // SAFETY: the buffer holds `count` ints.
        let r = unsafe { thread_info(port, THREAD_BASIC_INFO, info.as_mut_ptr(), &mut count) };
        r == 0 && info[6] == TH_STATE_RUNNING
    }

    pub(super) fn capture(t: &ThreadTarget, buf: &mut [u64]) -> Option<Captured> {
        let running = on_cpu(t.port);
        // SAFETY: a port for a thread of this process; a dead one fails.
        if unsafe { thread_suspend(t.port) } != 0 {
            return None;
        }
        let mut state = [0u64; 34];
        let mut count = regs::COUNT;
        // SAFETY: `state` has room for COUNT 32-bit words.
        let ok = unsafe {
            thread_get_state(t.port, regs::FLAVOR, state.as_mut_ptr().cast(), &mut count)
        } == 0;
        let mut depth = 0;
        let mut lr = 0;
        if ok {
            let pc = strip(state[regs::PC]);
            let sp = state[regs::SP];
            lr = if regs::LR < state.len() {
                strip(state[regs::LR])
            } else {
                0
            };
            buf[0] = pc;
            // The stack below SP is dead; never read it.
            let stack = (sp.max(t.stack.0), t.stack.1);
            // SAFETY: the target is suspended and `stack` is inside its
            // stack mapping, between its SP and its base.
            depth = unsafe { walk(state[regs::FP], stack, buf, 1) };
        }
        // SAFETY: balances the suspend above.
        unsafe { thread_resume(t.port) };
        ok.then_some(Captured {
            depth,
            lr,
            on_cpu: running,
        })
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[path = "sample_windows.rs"]
mod imp;

#[cfg(all(windows, not(target_arch = "x86_64")))]
mod imp {
    use super::{Captured, ThreadTarget};
    impl ThreadTarget {
        pub fn current(name: impl Into<String>) -> ThreadTarget {
            ThreadTarget {
                name: name.into(),
                port: 0,
                stack: (0, 0),
                handle: 0,
                cycles: Default::default(),
            }
        }
    }
    pub(super) fn capture(_: &ThreadTarget, _: &mut [u64]) -> Option<Captured> {
        None
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::{Captured, ThreadTarget};
    impl ThreadTarget {
        pub fn current(name: impl Into<String>) -> ThreadTarget {
            ThreadTarget {
                name: name.into(),
                port: 0,
                stack: (0, 0),
            }
        }
    }
    pub(super) fn capture(_: &ThreadTarget, _: &mut [u64]) -> Option<Captured> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walk_stops_at_the_stack_edges_and_on_loops() {
        // Three frame records laid out like a real stack: [next fp, ret].
        let mut stack = [0u64; 8];
        let p = stack.as_mut_ptr();
        let base = p as u64;
        let words = [base + 16, 0x1000, base + 32, 0x2000, base + 32, 0x3000];
        // (The last record points at itself: a loop.)
        for (i, w) in words.into_iter().enumerate() {
            // SAFETY: `i` < 8, inside `stack`.
            unsafe { p.add(i).write(w) };
        }
        let mut buf = [0u64; 8];
        let range = (base, base + 64);
        // SAFETY: `stack` is live and readable.
        let n = unsafe { walk(base, range, &mut buf, 0) };
        assert_eq!(&buf[..n], &[0x1000, 0x2000, 0x3000]);
        // An fp outside the range reads nothing.
        // SAFETY: as above; nothing outside `range` is read.
        assert_eq!(unsafe { walk(base + 64, range, &mut buf, 0) }, 0);
        // A short buffer caps the depth.
        let mut two = [0u64; 2];
        // SAFETY: as above.
        assert_eq!(unsafe { walk(base, range, &mut two, 0) }, 2);
    }

    #[test]
    fn stack_addresses_move_into_the_copy() {
        let mut w = [0x1000, 0x0fff, 0x1ff8, 0x2000, 7];
        relocate(&mut w, (0x1000, 0x2000), 0x9000);
        assert_eq!(w, [0x9000, 0x0fff, 0x9ff8, 0x2000, 7]);
    }

    #[test]
    fn pac_bits_are_stripped() {
        assert_eq!(strip(0x002f_0001_0000_1234), 0x0001_0000_1234);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn samples_a_busy_thread() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            tx.send(ThreadTarget::current("busy")).unwrap();
            let mut x = 0u64;
            while !flag.load(Ordering::Relaxed) {
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
            x
        });
        let target = rx.recv().unwrap();
        let s = Sampler::start(vec![target], 1000).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let p = s.finish();
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();
        // A loaded host can starve the sampler; only a few samples are needed.
        assert!(p.samples.len() >= 3, "{} samples", p.samples.len());
        assert!(
            p.samples
                .iter()
                .all(|s| !s.frames.is_empty() && s.frames[0] != 0)
        );
        // The busy loop is on the CPU nearly all the time, but a loaded
        // host can deschedule it for any share of a short run: only
        // require that the sampler saw it running.
        let on = p.samples.iter().filter(|s| s.on_cpu).count();
        assert!(on >= 1, "{on} of {} on cpu", p.samples.len());
        // The walk reaches past the leaf (thread start frames).
        assert!(p.samples.iter().any(|s| s.frames.len() > 2));
    }
}
