//! Copy maps to their cache slots several steps a loading frame
//! (docs/runtime/cache-slots.md, "Copy speed on the loading screen").
//!
//! Before a map is used it is copied from the DVD into a cache slot
//! (`points::CACHE_COPY_STEP`, the copy machine). The loading-screen loop
//! `0x163890` steps that machine once per iteration and draws and presents
//! a loading frame in between:
//!
//! ```text
//! loop { if job_done { copy_step() }; SwitchToThread(); draw and present }
//! ```
//!
//! Each step submits one job of at most 1 MiB (a read, then a write) and
//! the next step can only run once the job worker has finished it, so a map
//! copies at 1 MiB every two loading frames. On the console the DVD set
//! that pace. Here a read is a synchronous host read, done by the worker's
//! next slice, so the copy waits on frames instead: a 03a_oldmombasa boot
//! on fresh data spent 1,744 presents copying 838 MiB, and every one of
//! those frames costs whatever the loading screen costs to draw and show.
//!
//! This feature batches copy steps between loading frames,
//! yielding (`SwitchToThread`, as the loop itself does) between steps so the
//! worker finishes each job. The machine sees the same calls in the same
//! order with the same results, only with fewer frames drawn between them:
//! the copied bytes, the checksum and the slot header are the same. The
//! game itself steps the machine back to back with nothing drawn in its
//! flush loop `0x213fa0`. The loading-screen loop's call is boosted (its
//! return address [`LOADING_LOOP_RETURN`]). On a light host (a sim or a
//! dedicated host) the main loop's background copy is boosted the same way
//! ([`MAIN_LOOP_RETURN`], the sub-feature [`LOBBY`]): it is what the System
//! Link lobby waits on while the match's map copies into its slot. Elsewhere
//! the main loop keeps its pace. `H2_FEATURES=-map-copy` turns it all off.

use guest_layout::entry::{Cx, EntryFuture, EntryResult, GuestCall, Ret};
use hooks::{BuildError, Builder, Flow, Manifest};

pub const FEATURE: &str = "map-copy";

/// Where the loading-screen loop's call to the copy machine returns
/// (`0x1639bd` calls it).
pub const LOADING_LOOP_RETURN: u32 = 0x0016_39c2;
/// Where the main loop's call to the copy machine returns. The main loop
/// calls it at `0x12b56c` when the job-done byte is set (`0x12b564`) and
/// ignores what it returns.
pub const MAIN_LOOP_RETURN: u32 = 0x0012_b571;
/// The sub-feature boosting the main loop's call: on by default on a light
/// host (`profile_switches` in crates/apps/boot/src/registration.rs), off
/// elsewhere; `-map-copy.lobby` is its A/B arm.
pub const LOBBY: &str = "map-copy.lobby";
/// The copy job's done byte: cleared when a job is submitted (`0x1a1286`),
/// set by the worker when it finishes. Callers step the machine only while
/// it is set.
pub const JOB_DONE: u32 = 0x0055_bd0c;
/// The XAPI `SwitchToThread` (NtYieldExecution) the loop calls after each
/// step.
pub const SWITCH_TO_THREAD: u32 = 0x002d_2084;
/// Copy machine steps per loading frame: two per MiB, so about 8 MiB.
pub const STEPS: u32 = 16;
/// Destination slot chosen by the guest in state 7.
const COPY_SLOT: u32 = 0x0055_bd10;
/// Give the menu and its shared resources up to 64 MiB per loading frame.
const BOOT_STEPS: u32 = 128;
/// Yields to wait for one job before leaving the rest to the next frame.
pub const YIELDS: u32 = 8;

fn step_budget(ret: u32, fast_boot: bool, state: Option<u32>, slot: Option<u32>) -> u32 {
    if ret == LOADING_LOOP_RETURN
        && fast_boot
        && matches!(state, Some(11 | 12))
        && matches!(slot, Some(0..=2))
    {
        BOOT_STEPS
    } else {
        STEPS
    }
}

/// Whether a call to the copy machine returning to `ret` is boosted: the
/// loading screen's always, the main loop's with `lobby` ([`LOBBY`]).
pub fn boosted(ret: u32, lobby: bool) -> bool {
    ret == LOADING_LOOP_RETURN || (lobby && ret == MAIN_LOOP_RETURN)
}

/// How a call to the copy machine returning to `ret` runs: `None` as the
/// game makes it, else the boosted body with this many steps. The main
/// loop calls the machine every frame, idle too (the job-done byte stays
/// set), so an idle machine gets the original call there and no body: a
/// copy that call starts is boosted from the next frame.
fn plan_call(
    ret: u32,
    lobby: bool,
    fast_boot: bool,
    state: Option<u32>,
    slot: Option<u32>,
) -> Option<u32> {
    if !boosted(ret, lobby) || (ret == MAIN_LOOP_RETURN && state == Some(0)) {
        return None;
    }
    Some(step_budget(ret, fast_boot, state, slot))
}

/// The boosted call: the step the loop asked for, then up to `steps - 1`
/// more, each once the job before it is done, yielding as the loop does.
/// Stops when the machine is idle or a job is still running after `yields`
/// yields. Returns the last step's AL.
pub async fn steps(cx: Cx, steps: u32, yields: u32) -> EntryResult {
    let copy = GuestCall::new(guest_layout::points::CACHE_COPY_STEP.base);
    let mut eax = cx.call(copy.clone()).await?.eax;
    for _ in 1..steps {
        if cx.read(|m| m.u32(crate::map_slots::STATE))? == 0 {
            break;
        }
        let mut done = false;
        for _ in 0..yields {
            cx.call(GuestCall::new(SWITCH_TO_THREAD)).await?;
            if cx.read(|m| m.u8(JOB_DONE))? != 0 {
                done = true;
                break;
            }
        }
        if !done {
            break;
        }
        eax = cx.call(copy.clone()).await?.eax;
    }
    Ok(Ret::new(eax & 0xff, 0))
}

pub struct MapCopy;

impl hooks::Feature for MapCopy {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            FEATURE,
            "Copy maps to their cache slots several steps a loading frame",
        )
        .doc("docs/runtime/cache-slots.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let fast_boot = b.switches().feature("map-copy.boot", true);
        let lobby = b.switches().feature(LOBBY, false);
        b.raw().before(
            &guest_layout::points::CACHE_COPY_STEP,
            "several copy steps a loading frame",
            move |c| {
                let mem = c.ctx.runtime().memory();
                let ret = mem.u32(c.cpu.esp)?;
                if !boosted(ret, lobby) {
                    return Ok(Flow::Continue);
                }
                let Some(budget) = plan_call(
                    ret,
                    lobby,
                    fast_boot,
                    mem.u32(crate::map_slots::STATE).ok(),
                    mem.u32(COPY_SLOT).ok(),
                ) else {
                    return Ok(Flow::Continue);
                };
                drop(mem);
                // The steps it calls return to the body, not to the loop,
                // so they run the machine itself.
                c.answer_async(move |cx| -> EntryFuture { Box::pin(steps(cx, budget, YIELDS)) })
            },
        );
        // No state of its own: the copy is the game's, in guest memory.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_slots::STATE;
    use guest_layout::entry::{EntryError, Regs, testing};
    use guest_layout::memory::GuestMemory;
    use guest_layout::testing::FakeGuest;

    const MIB: usize = 1 << 20;

    /// A copy machine with the shape the loop sees: per chunk, a step that
    /// submits the read, then one that takes it and submits the write;
    /// a job completes on the worker's next slice (a yield). Synthetic
    /// bytes, no game data.
    #[derive(Clone, Debug, PartialEq)]
    struct Machine {
        source: Vec<u8>,
        at: usize,
        chunk: usize,
        buffer: Vec<u8>,
        slot: Vec<u8>,
        checksum: u32,
        /// 0 idle, 1 read submitted, 2 write submitted.
        state: u32,
        /// The submitted job, until a yield runs it.
        pending: bool,
        /// Every step's state on entry, in order.
        trace: Vec<u32>,
        /// Yields before a job completes (the worker's slices).
        slices: u32,
        waited: u32,
    }

    impl Machine {
        fn new(len: usize, chunk: usize) -> Self {
            Machine {
                source: (0..len).map(|i| (i * 31 + 7) as u8).collect(),
                at: 0,
                chunk,
                buffer: Vec::new(),
                slot: Vec::new(),
                checksum: 0,
                state: 1,
                pending: false,
                trace: Vec::new(),
                slices: 1,
                waited: 0,
            }
        }
        fn guest(&self) -> FakeGuest {
            let mut g = FakeGuest::new();
            g.put_u32(STATE, 0).put_u8(JOB_DONE, 1);
            g
        }
        fn submit(&mut self, m: &mut dyn GuestMemory) {
            self.pending = true;
            self.waited = 0;
            m.set_u8(JOB_DONE, 0).unwrap();
        }
        /// The copy machine. Steps only while the job-done byte is set.
        fn step(&mut self, m: &mut dyn GuestMemory) -> u32 {
            assert_eq!(m.u8(JOB_DONE).unwrap(), 1, "stepped with a job running");
            self.trace.push(self.state);
            match self.state {
                1 => {
                    let end = (self.at + self.chunk).min(self.source.len());
                    self.buffer = self.source[self.at..end].to_vec();
                    self.state = 2;
                    self.submit(m);
                }
                2 => {
                    for q in self.buffer.chunks(4) {
                        let mut w = [0u8; 4];
                        w[..q.len()].copy_from_slice(q);
                        self.checksum ^= u32::from_le_bytes(w);
                    }
                    self.slot.extend_from_slice(&self.buffer);
                    self.at += self.buffer.len();
                    self.state = if self.at < self.source.len() { 1 } else { 0 };
                    if self.state != 0 {
                        // Write submitted, next read after it.
                        self.submit(m);
                    }
                }
                s => panic!("stepped in state {s}"),
            }
            m.set_u32(STATE, self.state).unwrap();
            1
        }
        /// SwitchToThread: the worker's slice.
        fn yield_(&mut self, m: &mut dyn GuestMemory) {
            if self.pending {
                self.waited += 1;
                if self.waited >= self.slices {
                    self.pending = false;
                    m.set_u8(JOB_DONE, 1).unwrap();
                }
            }
        }
        fn callee(&mut self, c: &GuestCall, m: &mut dyn GuestMemory) -> Regs {
            let eax = match c.target {
                t if t == guest_layout::points::CACHE_COPY_STEP.base => self.step(m),
                SWITCH_TO_THREAD => {
                    self.yield_(m);
                    1
                }
                t => panic!("unexpected call to {t:#x}"),
            };
            Regs {
                eax,
                ..Default::default()
            }
        }
    }

    fn boxed(n: u32, y: u32) -> impl Fn(Cx) -> EntryFuture + Send + Sync {
        move |cx| Box::pin(steps(cx, n, y))
    }

    /// Runs the loading loop until the copy is idle: each frame steps the
    /// machine (once, or through the boosted body) if its job is done,
    /// then yields. Returns the machine and the frames drawn.
    fn load(mut m: Machine, boost: Option<(u32, u32)>) -> (Machine, u32) {
        let mut g = m.guest();
        g.put_u32(STATE, m.state);
        let mut frames = 0;
        while g.u32(STATE).unwrap() != 0 {
            frames += 1;
            assert!(frames < 100_000, "the copy never finished");
            if g.u8(JOB_DONE).unwrap() != 0 {
                match boost {
                    None => {
                        m.step(&mut g);
                    }
                    Some((n, y)) => {
                        let run = testing::run(&boxed(n, y), g, Regs::default(), |c, mem| {
                            m.callee(c, mem)
                        });
                        assert_eq!(run.result, Ok(Ret::new(1, 0)));
                        g = run.guest;
                    }
                }
            }
            m.yield_(&mut g);
        }
        (m, frames)
    }

    #[test]
    fn a_boosted_copy_makes_the_same_steps_and_bytes_in_fewer_frames() {
        let len = 20 * MIB + 12_345;
        let (plain, plain_frames) = load(Machine::new(len, MIB), None);
        let (boosted, boosted_frames) = load(Machine::new(len, MIB), Some((STEPS, YIELDS)));
        // Same results: every byte, the checksum, the machine's whole
        // sequence of steps.
        assert_eq!(boosted.slot, boosted.source);
        assert_eq!(boosted.slot, plain.slot);
        assert_eq!(boosted.checksum, plain.checksum);
        assert_eq!(boosted.trace, plain.trace);
        // Less waiting: 21 chunks, two steps each, one frame a step before;
        // STEPS a frame now.
        assert_eq!(plain_frames, 42);
        assert_eq!(boosted_frames, 42u32.div_ceil(STEPS));
    }

    #[test]
    fn menu_and_shared_payloads_need_fewer_presents_for_the_same_jobs() {
        for state in [11, 12] {
            for slot in 0..=2 {
                assert_eq!(
                    step_budget(LOADING_LOOP_RETURN, true, Some(state), Some(slot)),
                    BOOT_STEPS
                );
                // The main loop's call keeps the 16-step budget.
                assert_eq!(
                    step_budget(MAIN_LOOP_RETURN, true, Some(state), Some(slot)),
                    STEPS
                );
            }
        }
        for (enabled, state, slot) in [
            (false, Some(11), Some(0)),
            (true, Some(11), Some(3)),
            (true, Some(7), Some(2)),
            (true, None, None),
        ] {
            assert_eq!(
                step_budget(LOADING_LOOP_RETURN, enabled, state, slot),
                STEPS
            );
        }
        let source = Machine::new(128 * 1024 + 7, 1024);
        let (previous, old_frames) = load(source.clone(), Some((STEPS, YIELDS)));
        let budget = step_budget(LOADING_LOOP_RETURN, true, Some(12), Some(2));
        let (fast, frames) = load(source, Some((budget, YIELDS)));
        assert_eq!(fast, previous);
        assert_eq!((old_frames, frames), (17, 3));
    }

    #[test]
    fn a_slow_worker_still_copies_everything_the_same() {
        let len = 5 * MIB;
        let mut slow = Machine::new(len, MIB);
        slow.slices = 3;
        let (plain, plain_frames) = load(slow.clone(), None);
        let (boosted, boosted_frames) = load(slow, Some((STEPS, YIELDS)));
        assert_eq!(
            (boosted.slot.clone(), boosted.trace.clone()),
            (plain.slot, plain.trace)
        );
        assert!(
            boosted_frames < plain_frames,
            "{boosted_frames} vs {plain_frames}"
        );
    }

    #[test]
    fn a_job_that_outlasts_the_yields_leaves_the_rest_to_the_next_frame() {
        let mut m = Machine::new(4 * MIB, MIB);
        m.slices = YIELDS + 1;
        let mut g = m.guest();
        g.put_u32(STATE, 1);
        let run = testing::run(&boxed(STEPS, YIELDS), g, Regs::default(), |c, mem| {
            m.callee(c, mem)
        });
        assert_eq!(run.result, Ok(Ret::new(1, 0)));
        let copy = GuestCall::new(guest_layout::points::CACHE_COPY_STEP.base);
        let mut want = vec![copy];
        want.extend((0..YIELDS).map(|_| GuestCall::new(SWITCH_TO_THREAD)));
        assert_eq!(run.calls, want);
        assert_eq!(m.trace, [1]);
    }

    #[test]
    fn nothing_more_runs_once_the_machine_is_idle() {
        // Half a chunk: read, then write and idle.
        let mut m = Machine::new(MIB / 2, MIB);
        let mut g = m.guest();
        g.put_u32(STATE, 1);
        let run = testing::run(&boxed(STEPS, YIELDS), g, Regs::default(), |c, mem| {
            m.callee(c, mem)
        });
        assert_eq!(run.result, Ok(Ret::new(1, 0)));
        assert_eq!(m.trace, [1, 2]);
        assert_eq!(run.guest.u32(STATE), Ok(0));
        assert_eq!(
            run.calls.last(),
            Some(&GuestCall::new(guest_layout::points::CACHE_COPY_STEP.base))
        );
        assert_eq!(m.slot, m.source);
    }

    #[test]
    fn one_step_is_the_original_call() {
        let mut m = Machine::new(4 * MIB, MIB);
        let mut g = m.guest();
        g.put_u32(STATE, 1);
        let run = testing::run(&boxed(1, YIELDS), g, Regs::default(), |c, mem| {
            m.callee(c, mem)
        });
        assert_eq!(
            run.calls,
            [GuestCall::new(guest_layout::points::CACHE_COPY_STEP.base)]
        );
        assert_eq!(run.result, Ok(Ret::new(1, 0)));
    }

    #[test]
    fn a_fault_reading_the_machine_ends_the_call() {
        // Nothing mapped: the first step runs, the state read faults.
        let run = testing::run(
            &boxed(STEPS, YIELDS),
            FakeGuest::new(),
            Regs::default(),
            |_, _| Regs::default(),
        );
        assert!(matches!(run.result, Err(EntryError::Fault(_))));
        assert_eq!(run.calls.len(), 1);
    }

    #[test]
    fn the_feature_plans_one_before_hook_on_the_copy_machine() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&MapCopy).unwrap();
        let plan = r.build(&hooks::Base).unwrap_or_else(|e| panic!("{e:?}"));
        let text = plan.text();
        assert!(text.contains("CACHE_COPY_STEP"), "{text}");
        assert!(
            text.contains("several copy steps a loading frame"),
            "{text}"
        );
        // Switched off, nothing is hooked.
        let mut r = hooks::Registry::new(hooks::Switches::parse("-map-copy").unwrap());
        r.add(&MapCopy).unwrap();
        let plan = r.build(&hooks::Base).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!plan.text().contains("CACHE_COPY_STEP"), "{}", plan.text());
    }

    #[test]
    fn the_main_loop_is_boosted_only_with_the_lobby_switch() {
        assert!(boosted(LOADING_LOOP_RETURN, false));
        assert!(boosted(LOADING_LOOP_RETURN, true));
        // The main loop's call (0x12b56c) on a light host only.
        assert_eq!(MAIN_LOOP_RETURN, 0x0012_b571);
        assert!(!boosted(MAIN_LOOP_RETURN, false));
        assert!(boosted(MAIN_LOOP_RETURN, true));
        // The flush loop's (0x213fc0) never.
        assert!(!boosted(0x0021_3fc5, false));
        assert!(!boosted(0x0021_3fc5, true));
    }

    #[test]
    fn an_idle_machine_gets_the_main_loop_call_as_it_is() {
        // Idle: the original call, every frame of a match.
        assert_eq!(
            plan_call(MAIN_LOOP_RETURN, true, true, Some(0), Some(3)),
            None
        );
        // Copying: the 16-step budget, whatever the slot.
        for slot in 0..=5 {
            assert_eq!(
                plan_call(MAIN_LOOP_RETURN, true, true, Some(12), Some(slot)),
                Some(STEPS)
            );
        }
        assert_eq!(
            plan_call(MAIN_LOOP_RETURN, false, true, Some(12), Some(3)),
            None
        );
        // The loading screen's call is boosted as before, idle or not.
        assert_eq!(
            plan_call(LOADING_LOOP_RETURN, false, true, Some(0), None),
            Some(STEPS)
        );
        assert_eq!(
            plan_call(LOADING_LOOP_RETURN, false, true, Some(12), Some(2)),
            Some(BOOT_STEPS)
        );
        assert_eq!(plan_call(0x0021_3fc5, true, true, Some(12), Some(3)), None);
    }

    /// The System Link lobby's wait: the main loop steps the machine once a
    /// frame while the job-done byte is set (`0x12b564`), the worker
    /// finishing a job between frames. Boosted from the main loop's return
    /// address, the slot 3 copy makes the same steps in the same order with
    /// the same bytes and checksum, in fewer frames.
    #[test]
    fn a_boosted_lobby_copy_makes_the_same_steps_and_bytes_in_fewer_frames() {
        // Lockout-sized payload in the multiplayer slot, a copy in state 12.
        let budget = step_budget(MAIN_LOOP_RETURN, true, Some(12), Some(3));
        assert_eq!(budget, STEPS);
        let len = 40 * MIB + 777;
        let (plain, plain_frames) = load(Machine::new(len, MIB), None);
        let (boosted, boosted_frames) = load(Machine::new(len, MIB), Some((budget, YIELDS)));
        assert_eq!(boosted.slot, boosted.source);
        assert_eq!(boosted.slot, plain.slot);
        assert_eq!(boosted.checksum, plain.checksum);
        assert_eq!(boosted.trace, plain.trace);
        // 41 chunks, two steps each: a frame a step before, 16 a frame now.
        assert_eq!(plain_frames, 82);
        assert_eq!(boosted_frames, 82u32.div_ceil(STEPS));
        // A worker slower than a frame still copies the same.
        let mut slow = Machine::new(3 * MIB, MIB);
        slow.slices = 2;
        let (plain, plain_frames) = load(slow.clone(), None);
        let (boosted, boosted_frames) = load(slow, Some((budget, YIELDS)));
        assert_eq!(
            (&boosted.slot, &boosted.trace, boosted.checksum),
            (&plain.slot, &plain.trace, plain.checksum)
        );
        assert!(
            boosted_frames < plain_frames,
            "{boosted_frames} vs {plain_frames}"
        );
    }

    #[test]
    fn the_lobby_switch_keeps_the_hook_on_the_copy_machine() {
        for spec in ["map-copy.lobby", "-map-copy.lobby"] {
            let mut r = hooks::Registry::new(hooks::Switches::parse(spec).unwrap());
            r.add(&MapCopy).unwrap();
            let plan = r.build(&hooks::Base).unwrap_or_else(|e| panic!("{e:?}"));
            let text = plan.text();
            assert!(
                text.contains("several copy steps a loading frame"),
                "{spec}: {text}"
            );
        }
    }
}
