//! A phantom is local user zero in a private copy of the host world.
//! Only its pad history, player-control history and returned record survive.
//!
//! The private copy is one of two paths (docs/perf/sim-phantom-build.md),
//! picked by `impl.sim.phantom-alias` ([`path_for`]): on by default, the
//! build runs on live RAM through [`SecondExecutor::alias`] and its write
//! journal puts every byte back after each seat; off, and on a flat-RAM
//! image, it runs in a copy-on-write snapshot taken at the tick's first
//! seat. `H2_PHANTOM_BUILD_CHECK=1` runs both for every seat and counts the
//! seats whose outcome, action or history differ.
//!
//! The input stage (the bind, the look rates, the pad read and the remap)
//! always runs in Rust (`engine::sim_seats::input`); a piece that refuses
//! is counted by name and runs its guest entry instead.
//!
//! The builder `0x186ab0` itself, the interaction search under it and the
//! seats under those always run in Rust ([`body`]); a segment that declines
//! is counted and runs the original from there. [`Stage::Guest`] and a
//! builder without the body are the tests' and the check mode's lifted
//! reference only.
use anyhow::Result;
use bots::phantom::{SeatInput, session::at::ACTION_SIZE};
use engine::sim_seats::input::{self as seat_input, Refusal};
use runtime::{
    CopyKind, CpuAbi, Guest, Runtime,
    executor::{Alias, ExecutorError, SecondExecutor},
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

mod animation;
mod body;
use animation::AnimationRead;

/// Game-thread nanoseconds in [`Builder::sample`] since the process
/// began: every seat's build, snapshot and reverts included. `h2 sim` reports the delta over a match
/// as `wall.phantom_build_ms_per_tick` (docs/runtime/sim.md §6).
static BUILD_NS: AtomicU64 = AtomicU64::new(0);
/// The part of [`BUILD_NS`] in [`SecondExecutor::snapshot`].
static SNAPSHOT_NS: AtomicU64 = AtomicU64::new(0);
/// Snapshots that remapped guest RAM, and that copied it.
static SNAPSHOTS_REMAPPED: AtomicU64 = AtomicU64::new(0);
static SNAPSHOTS_COPIED: AtomicU64 = AtomicU64::new(0);
/// Check mode (`H2_PHANTOM_BUILD_CHECK=1`): seats built on both paths, and
/// those whose outcome, action or history differed.
static CHECKED: AtomicU64 = AtomicU64::new(0);
static MISMATCHES: AtomicU64 = AtomicU64::new(0);
/// The per-seat split of [`BUILD_NS`] (docs/runtime/sim.md §6): the setup
/// writes, each guest call (`sim_bench::metrics::PHANTOM_CALLS` order)
/// with its guest accesses, `written` plus `revert`, and the read back.
static SETUP_NS: AtomicU64 = AtomicU64::new(0);
static CALL_NS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static CALL_ACCESSES: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static REVERT_NS: AtomicU64 = AtomicU64::new(0);
static READ_NS: AtomicU64 = AtomicU64::new(0);
/// Pages the reverted seats had written, and how many seats that is.
static PAGES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static PAGES_SEATS: AtomicU64 = AtomicU64::new(0);
/// Minor faults the process took inside [`Builder::sample`].
static MINOR_FAULTS: AtomicU64 = AtomicU64::new(0);

fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

fn load6(counters: &[AtomicU64; 6]) -> [u64; 6] {
    std::array::from_fn(|i| counters[i].load(Ordering::Relaxed))
}

/// The process's minor page faults so far; 0 where there is no `getrusage`
/// (Windows).
#[cfg(not(unix))]
fn minor_faults() -> u64 {
    0
}

/// The process's minor page faults so far (`getrusage`).
#[cfg(unix)]
fn minor_faults() -> u64 {
    // SAFETY: `getrusage` fills a zeroed `rusage` we own.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0;
    }
    usage.ru_minflt.max(0) as u64
}

fn elapsed_ns(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// The phantoms' action-build totals since the process began, for the
/// sim report's `wall.phantom_*` fields.
pub(crate) fn build_costs() -> sim_bench::metrics::PhantomCosts {
    sim_bench::metrics::PhantomCosts {
        build_ns: BUILD_NS.load(Ordering::Relaxed),
        snapshot_ns: SNAPSHOT_NS.load(Ordering::Relaxed),
        remapped: SNAPSHOTS_REMAPPED.load(Ordering::Relaxed),
        copied: SNAPSHOTS_COPIED.load(Ordering::Relaxed),
        checked: CHECKED.load(Ordering::Relaxed),
        mismatches: MISMATCHES.load(Ordering::Relaxed),
        setup_ns: SETUP_NS.load(Ordering::Relaxed),
        call_ns: load6(&CALL_NS),
        call_accesses: load6(&CALL_ACCESSES),
        revert_ns: REVERT_NS.load(Ordering::Relaxed),
        read_ns: READ_NS.load(Ordering::Relaxed),
        pages_written: PAGES_WRITTEN.load(Ordering::Relaxed),
        pages_seats: PAGES_SEATS.load(Ordering::Relaxed),
        minor_faults: MINOR_FAULTS.load(Ordering::Relaxed),
    }
}

fn note_snapshot(since: Instant, kind: CopyKind) {
    SNAPSHOT_NS.fetch_add(elapsed_ns(since), Ordering::Relaxed);
    match kind {
        CopyKind::Remapped => &SNAPSHOTS_REMAPPED,
        CopyKind::Copied => &SNAPSHOTS_COPIED,
    }
    .fetch_add(1, Ordering::Relaxed);
}

/// How a seat's build reaches guest RAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Path {
    /// On live RAM ([`SecondExecutor::alias`]), every store journaled and
    /// undone before the seat's gather returns.
    Alias,
    /// In a copy-on-write snapshot of the tick's first gather
    /// ([`SecondExecutor::snapshot`]), reverted between seats.
    View,
}

/// The path `impl.sim.phantom-alias` picks: the alias unless it is off.
pub(super) fn path_for(switches: &hooks::Switches) -> Path {
    if switches.implementation("sim.phantom-alias", true) {
        Path::Alias
    } else {
        Path::View
    }
}

/// Who runs the input stage: the four entries `0x1872d0`, `0x190c34`,
/// `0x124c00` and `0x217c40`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    /// `engine::sim_seats::input`, each piece falling back to its guest
    /// entry when it refuses.
    Rust,
    /// The guest entries.
    Guest,
}

/// Input-stage pieces that refused and ran their guest entry.
static REFUSED: AtomicU64 = AtomicU64::new(0);

/// `H2_PHANTOM_BUILD_CHECK=1`: build every seat on both paths and compare.
fn check_mode() -> bool {
    std::env::var("H2_PHANTOM_BUILD_CHECK").is_ok_and(|v| v == "1")
}

/// How far below the gather's ESP the alias path puts the stack back to
/// the tick's first gather for a later seat (below). The builder's frames
/// sit under the gather frame; 64 KiB is the window, not a measured depth.
const STACK_WINDOW: u32 = 0x10000;
const STACK_PAGE: u32 = 0x1000;

/// The game thread's stack pages around the gather's ESP, as the tick's
/// first gather left them: the view path shows a later seat these bytes,
/// so the alias path writes them back (journaled) where live RAM has
/// moved on. Unmapped pages are left out.
fn stack_window(m: &Guest, esp: u32) -> Vec<(u32, Box<[u8; STACK_PAGE as usize]>)> {
    let lo = u64::from(esp.saturating_sub(STACK_WINDOW) & !(STACK_PAGE - 1));
    let above = u64::from(esp) | u64::from(STACK_PAGE - 1);
    let hi = (above + 1 + u64::from(STACK_PAGE)).min(1 << 32);
    (lo..hi)
        .step_by(STACK_PAGE as usize)
        .filter_map(|va| {
            let va = va as u32;
            m.read::<{ STACK_PAGE as usize }>(va).ok().map(|b| (va, Box::new(b)))
        })
        .collect()
}

pub(super) const WORK_BYTES: u32 = 0x1000;
const RAW: u32 = 0x4e61dc;
const OBSERVER: u32 = 0x4e9c8c;
const DIRECTOR: u32 = 0x4e8c44;
const ANIMATION_RESOURCE_REQUEST: u32 = 0x1236f0;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum BuildFailure {
    Refused(u32),
    Fault,
    Cancelled,
    Stack,
    NoSnapshot,
    Memory,
    ColdAnimation,
}

impl BuildFailure {
    fn kind(err: &anyhow::Error) -> Self {
        match err.downcast_ref::<ExecutorError>() {
            Some(ExecutorError::Refused(r)) => Self::Refused(r.target),
            Some(ExecutorError::Fault { .. }) => Self::Fault,
            Some(ExecutorError::Cancelled) => Self::Cancelled,
            Some(ExecutorError::Stack(_)) => Self::Stack,
            Some(ExecutorError::NoSnapshot) => Self::NoSnapshot,
            None => Self::Memory,
        }
    }
}

#[derive(PartialEq, Eq)]
struct History {
    raw: [u8; 0x48],
    control: [u8; 0x94],
    layout: [u8; 0x50],
    latch: u8,
}

pub(super) struct Builder {
    exec: SecondExecutor,
    workspace: u32,
    tick: Option<u32>,
    history: BTreeMap<u16, History>,
    reported: BTreeSet<BuildFailure>,
    cold: animation::ColdRead,
    path: Path,
    stage: Stage,
    /// Input-stage refusals already said.
    refused: BTreeSet<Refusal>,
    /// The builder's body runs in Rust ([`Builder::with_body`]).
    body: bool,
    /// Alias path: the stack window of the tick's first gather.
    stack: Vec<(u32, Box<[u8; STACK_PAGE as usize]>)>,
    /// Check mode: the same seats built on the other path with the guest
    /// input stage, with their own executor and history.
    other: Option<Box<Builder>>,
}

/// The seat's action, built with the input stage and the builder body in
/// Rust ([`gather_with`]).
pub(super) async fn gather(
    input: &Mutex<Option<Builder>>,
    main: &Runtime,
    workspace: u32,
    path: Path,
    cpu: CpuAbi,
    seat: SeatInput,
    cx: &guest_layout::entry::Cx,
) -> Option<[u8; ACTION_SIZE]> {
    gather_with(input, main, workspace, path, Stage::Rust, true, cpu, seat, cx).await
}

/// [`gather`] with the stage and body named: the tests build on the lifted
/// stage as their reference. Only the live animation cache may submit or wait for a resource read.
/// Retry the pad against fresh RAM once after a synchronous cache fill.
pub(super) async fn gather_with(
    input: &Mutex<Option<Builder>>,
    main: &Runtime,
    workspace: u32,
    path: Path,
    stage: Stage,
    body: bool,
    cpu: CpuAbi,
    seat: SeatInput,
    cx: &guest_layout::entry::Cx,
) -> Option<[u8; ACTION_SIZE]> {
    let attempt = |refresh| {
        let mut guard = input.lock().unwrap_or_else(|e| e.into_inner());
        let builder = guard.get_or_insert_with(|| {
            Builder::new(main, workspace, path, stage).with_body(main, body)
        });
        if refresh {
            builder.restart();
        }
        let faults = minor_faults();
        let started = Instant::now();
        let outcome = builder.sample_checked(main, cpu, seat);
        BUILD_NS.fetch_add(elapsed_ns(started), Ordering::Relaxed);
        add(&MINOR_FAULTS, minor_faults().saturating_sub(faults));
        outcome
    };
    let read = match attempt(false) {
        Ok(action) => return action,
        Err(read) => read,
    };
    if let Some(call) = cx.read(|m| read.live_call(m)) {
        cx.call(call).await.ok()?;
        match attempt(true) {
            Ok(action) => return action,
            Err(read) => report_cold(input, read, seat),
        }
    } else {
        report_cold(input, read, seat);
    }
    None
}

fn report_cold(input: &Mutex<Option<Builder>>, read: AnimationRead, seat: SeatInput) {
    let mut guard = input.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(builder) = guard.as_mut()
        && builder.reported.insert(BuildFailure::ColdAnimation)
    {
        eprintln!(
            "phantoms: missed action for player {} at tick {}: animation resource {:#x} unavailable",
            seat.player.index(),
            seat.tick,
            read.record
        );
    }
}

impl Builder {
    /// A builder on `path`, or on the view where the alias cannot run (a
    /// flat-RAM image, said once). In check mode it carries a second
    /// builder on the other path with the guest input stage: the
    /// reference arm.
    fn new(main: &Runtime, workspace: u32, path: Path, stage: Stage) -> Self {
        let usable = |path| {
            if path == Path::Alias && main.flat_ram() {
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| {
                    eprintln!(
                        "phantoms: a flat-RAM image builds actions in a copy-on-write view; impl.sim.phantom-alias needs the paged memory model"
                    );
                });
                Path::View
            } else {
                path
            }
        };
        let path = usable(path);
        let mut builder = Self::on(main, workspace, path, stage);
        if check_mode() {
            let other = usable(match path {
                Path::Alias => Path::View,
                Path::View => Path::Alias,
            });
            if other != path || stage != Stage::Guest {
                builder.other = Some(Box::new(Self::on(main, workspace, other, Stage::Guest)));
            }
        }
        builder
    }

    fn on(main: &Runtime, workspace: u32, path: Path, stage: Stage) -> Self {
        let exec = SecondExecutor::new(main, |_, _| false);
        exec.runtime().edit_dispatch(|d| {
            d.prefer_generated();
            // Weapon and animation lookups predict resources after choosing an
            // animation. Queuing I/O here would post a job to a private queue
            // with no worker. The prediction returns void and takes no stack
            // arguments. Actual resource reads still require resident data.
            d.register_override(
                ANIMATION_RESOURCE_REQUEST,
                "phantom: skip animation prefetch",
                |cpu, ctx, _| ctx.memory().ret(cpu, 0),
            );
            d.register_override(
                xapi::register::addr::XINPUT_GET_STATE,
                "seat XInput report",
                move |cpu, ctx, _| {
                    let mut m = ctx.memory();
                    let out = m.u32(cpu.esp + 8)?;
                    let report = m.read::<{ input::State::SIZE }>(workspace)?;
                    m.write(out, report)?;
                    cpu.eax = 0;
                    m.ret(cpu, 8)
                },
            );
        });
        let cold = animation::intercept_cold_reads(exec.runtime());
        Self {
            exec,
            workspace,
            tick: None,
            history: Default::default(),
            reported: BTreeSet::new(),
            cold,
            path,
            stage,
            refused: BTreeSet::new(),
            body: false,
            stack: Vec::new(),
            other: None,
        }
    }

    /// With `on`, the executor runs the builder `0x186ab0`, the
    /// interaction search and the seats under them in Rust ([`body`]).
    /// The check mode's reference arm keeps the guest's; where the path
    /// and stage alone gave it none (a flat-RAM image with the guest
    /// input stage), it gets one on the same path.
    fn with_body(mut self, main: &Runtime, on: bool) -> Self {
        if on && !self.body {
            self.exec.runtime().edit_dispatch(body::install);
            self.body = true;
            if check_mode() && self.other.is_none() {
                self.other = Some(Box::new(Self::on(main, self.workspace, self.path, Stage::Guest)));
            }
        }
        self
    }

    /// The next seat starts from live RAM, on both paths in check mode.
    fn restart(&mut self) {
        self.tick = None;
        if let Some(other) = &mut self.other {
            other.tick = None;
        }
    }

    /// [`sample`](Self::sample), and in check mode the other path's sample
    /// of the same seat first: a seat whose outcome, action bytes or
    /// history differ is counted in `wall.phantom_build_mismatches`. The
    /// switch's path gives the answer; each path keeps its own history, so
    /// each runs as it would alone.
    fn sample_checked(
        &mut self,
        main: &Runtime,
        cpu: CpuAbi,
        seat: SeatInput,
    ) -> Result<Option<[u8; ACTION_SIZE]>, AnimationRead> {
        let Some(mut other) = self.other.take() else {
            return self.sample(main, cpu, seat);
        };
        let theirs = other.sample(main, cpu, seat);
        let ours = self.sample(main, cpu, seat);
        let idx = seat.player.index();
        let same = match (&ours, &theirs) {
            (Ok(a), Ok(b)) => a == b,
            (Err(a), Err(b)) => a == b,
            _ => false,
        } && self.history.get(&idx) == other.history.get(&idx);
        CHECKED.fetch_add(1, Ordering::Relaxed);
        if !same {
            let n = MISMATCHES.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 8 {
                eprintln!(
                    "phantoms: the {:?} ({:?} input, Rust body {}) and {:?} ({:?} input, Rust body {}) builds differ for player {} at tick {} (mismatch {n})",
                    self.path,
                    self.stage,
                    self.body,
                    other.path,
                    other.stage,
                    other.body,
                    seat.player.index(),
                    seat.tick
                );
            }
        }
        self.other = Some(other);
        ours
    }

    /// An isolated failure is a missing controller sample at the gather hook.
    fn sample(
        &mut self,
        main: &Runtime,
        cpu: CpuAbi,
        seat: SeatInput,
    ) -> Result<Option<[u8; ACTION_SIZE]>, AnimationRead> {
        match self.action(main, cpu, seat) {
            Ok(action) => Ok(Some(action)),
            Err(err) => {
                let cold = self.cold.lock().unwrap_or_else(|e| e.into_inner()).take();
                if cold.is_none() && self.reported.insert(BuildFailure::kind(&err)) {
                    let trace = self
                        .exec
                        .refused_cpu()
                        .map(|at| self.exec.context().backtrace(at.eip));
                    eprintln!(
                        "phantoms: missed action for player {} at tick {}: {err:#} (private call chain {trace:x?})",
                        seat.player.index(),
                        seat.tick
                    );
                }
                // Abandon the private stack and its callbacks. The next seat
                // starts from live RAM, even if it gathers in this same tick.
                self.exec.context().clear_continuations();
                self.exec.context().clear_backtrace();
                self.tick = None;
                match cold {
                    Some(read) => Err(read),
                    None => Ok(None),
                }
            }
        }
    }

    /// View path: a snapshot at the tick's first seat; later seats revert
    /// what the earlier ones wrote, so each sees the same world, even when
    /// the search wrote scratch globals.
    fn view(&mut self, main: &Runtime, first: bool) {
        if first {
            let started = Instant::now();
            let kind = self.exec.snapshot(main);
            note_snapshot(started, kind);
        } else {
            // Each seat sees the same world, even when the search wrote scratch globals.
            let started = Instant::now();
            let pages = self.exec.written();
            for page in &pages {
                self.exec.revert(page.physical, runtime::guest::PAGE_SIZE);
            }
            add(&REVERT_NS, elapsed_ns(started));
            add(&PAGES_WRITTEN, pages.len() as u64);
            add(&PAGES_SEATS, 1);
        }
    }

    /// Alias path: live RAM, held from here through the undo. Every seat's
    /// writes are undone before its gather returns, so a later seat never
    /// sees an earlier one's. The tick's first seat starts the executor's
    /// clock and time-stamp counter as a snapshot does and keeps the stack
    /// window; a later seat runs them on, as the view's later seats do, and
    /// gets the first gather's stack bytes back where they moved.
    fn alias<'m>(&mut self, main: &'m Runtime, first: bool, esp: u32) -> Result<Alias<'m>> {
        let alias = if first {
            self.exec.alias(main)?
        } else {
            self.exec.alias_continued(main)?
        };
        let mut m = self.exec.runtime().memory();
        if first {
            self.stack = stack_window(&m, esp);
        } else {
            for (va, bytes) in &self.stack {
                if m.read::<{ STACK_PAGE as usize }>(*va).is_ok_and(|now| now != **bytes) {
                    m.write(*va, **bytes)?;
                }
            }
        }
        drop(m);
        Ok(alias)
    }

    /// One input-stage piece in Rust, on the registers its guest call
    /// would start with, timed in that call's slot of the split. False
    /// when the stage is the guest's or the piece refused (each refusal
    /// said once): the caller then makes the guest call, from the RAM the
    /// refusal left untouched.
    fn rust(
        &mut self,
        entry: u32,
        regs: &CpuAbi,
        piece: impl FnOnce(&mut Guest, &CpuAbi) -> Result<(), Refusal>,
    ) -> bool {
        if self.stage != Stage::Rust {
            return false;
        }
        let role = match entry {
            seat_input::BIND => 0,
            seat_input::LOOK_RATES => 1,
            seat_input::READ_PADS => 3,
            _ => 4,
        };
        let started = Instant::now();
        let outcome = piece(&mut self.exec.runtime().memory(), regs);
        add(&CALL_NS[role], elapsed_ns(started));
        let Err(refusal) = outcome else { return true };
        add(&REFUSED, 1);
        if self.refused.insert(refusal) {
            eprintln!(
                "phantoms: the input stage refused {} at {entry:#x}; that piece runs the guest entry",
                refusal.name()
            );
        }
        false
    }

    fn action(&mut self, main: &Runtime, cpu: CpuAbi, s: SeatInput) -> Result<[u8; ACTION_SIZE]> {
        let idx = s.player.index();
        if s.reset {
            self.history.remove(&idx);
        }
        let first = self.tick != Some(s.tick) || s.reset;
        // Alias path: undone as this drops, after the read back below, on
        // a faulted or refused call's early return, and on a panic.
        let _alias = match self.path {
            Path::Alias => Some(self.alias(main, first, cpu.esp)?),
            Path::View => {
                self.view(main, first);
                None
            }
        };
        self.tick = Some(s.tick);
        let work = self.workspace;
        let ctrl;
        let setup_started = Instant::now();
        {
            let mut m = self.exec.runtime().memory();
            let users = m.u32(0x4e8c20)?;
            m.set_u32(users + 8, 1)?;
            m.write(users + 12, [0xff; 32])?;
            m.set_u32(users + 12, s.player.raw())?;
            m.set_u32(users + 0x1c, s.player.raw())?;
            m.set_u32(s.player_addr + 0x24, 0)?;
            m.set_u16(s.player_addr + 0x28, 0)?;
            m.write(0x4e61cc, [0; 16])?;
            m.set_u32(0x4e61cc, 1)?;
            m.set_u8(0x4e61b9, 0)?;
            m.write(work, s.pad.to_bytes())?;
            ctrl = m.u32(0x4ed284)? + 0x14;
            // A gameplay director, no menu input or observer belonging to another player.
            m.write(DIRECTOR, [0; 0x140])?;
            m.set_u16(0x4686c4, 0)?;
            // The phantom has no open dialogs. Clear the port and global screen
            // lists checked by 0x148fff in this view only.
            for list in [0x54d884, 0x54d76c, 0x54d62c, 0x54d99c, 0x54dba8, 0x54d9e0] {
                m.write(list + 8, [0; 8])?;
            }
            // On a playerless host slot zero has no profile or local-user index.
            // Supply the same profile binding as a signed-in bot in the private view.
            let slot = bots::guest::slot(0);
            m.set_u32(slot, bots::guest::SLOT_PROFILE)?;
            m.set_u32(slot + bots::guest::SLOT_LOCAL_USER, 0)?;
            m.write(0x54e9f4, [0, 0, 0, 0, 0, 0, bots::guest::BOT_SENSITIVITY])?;
            if let Some(h) = self.history.get(&idx) {
                m.write(RAW, h.raw)?;
                m.write(ctrl, h.control)?;
                m.write(0x51ea88, h.layout)?;
                m.set_u8(0x51ebcc, h.latch)?;
            } else {
                m.write(RAW, [0; 0x48])?;
                m.write(0x51ea88, [0; 0x50])?;
                m.set_u8(0x51ebcc, 0)?;
                // Discard user zero's previous shot-assist block
                // before retail initializes control for the newly bound unit.
                let assist = ctrl + crate::touch_glue::PC_TARGET;
                let mut empty = [0; 0x24];
                empty[..12].fill(0xff); // object, marker and controlling player
                m.write(assist, empty)?;
                m.set_u32(ctrl, u32::MAX)?;
            }
        }
        add(&SETUP_NS, elapsed_ns(setup_started));
        // The COW view includes the calling game thread's stack and FS. The
        // guest calls can push below the gather frame without touching it.
        let stack = cpu.esp;
        // `role` indexes `sim_bench::metrics::PHANTOM_CALLS`.
        let call = |exec: &mut SecondExecutor,
                    role: usize,
                    addr,
                    eax,
                    ecx,
                    args: &[u32]|
         -> Result<u32> {
            let mut regs = CpuAbi {
                esp: stack,
                eax,
                ecx,
                ..cpu
            };
            let started = Instant::now();
            let outcome = exec.call(&mut regs, addr, args);
            add(&CALL_NS[role], elapsed_ns(started));
            add(&CALL_ACCESSES[role], outcome?.accesses);
            Ok(regs.eax)
        };
        let regs = |eax, ecx| CpuAbi { esp: stack, eax, ecx, ..cpu };
        let unit = s.unit.raw();
        // Bind or reset control if the unit changed.
        if !self.rust(seat_input::BIND, &regs(0, unit), |m, c| seat_input::bind(m, c, 0, unit)) {
            call(&mut self.exec, 0, seat_input::BIND, 0, unit, &[])?;
        }
        // Profile to look rates over the Default layout.
        if !self.rust(seat_input::LOOK_RATES, &regs(0, 0), |m, _| seat_input::look_rates(m, 0)) {
            call(&mut self.exec, 1, seat_input::LOOK_RATES, 0, 0, &[])?;
        }
        let director = call(
            &mut self.exec,
            2,
            0x1557c0,
            s.unit.raw(),
            0,
            &[OBSERVER, work + 0x20],
        )?;
        self.exec.runtime().memory().set_u32(
            DIRECTOR + 12,
            if director as u16 == 0 {
                0x23c110
            } else {
                0x23cbb0
            },
        )?;
        // Exactly the reader's milliseconds, including the fractional tick over time.
        let ms = |t: u64| (t as f64 * f64::from(s.dt) * 1000.).round() as u64;
        let elapsed = ms(u64::from(s.tick) + 1).saturating_sub(ms(u64::from(s.tick))) as u32;
        // The pad as the executor's XInputGetState answers it.
        let read = |m: &mut Guest, _: &CpuAbi| -> Result<(), Refusal> {
            let report = <Guest as engine::sim_native::Memory>::load::<{ seat_input::REPORT }>(m, work)?;
            seat_input::read_pads(m, elapsed, &report)
        };
        if !self.rust(seat_input::READ_PADS, &regs(0, 0), read) {
            call(&mut self.exec, 3, seat_input::READ_PADS, 0, 0, &[elapsed])?;
        }
        if !self.rust(seat_input::REMAP, &regs(0, 0), seat_input::remap) {
            call(&mut self.exec, 4, seat_input::REMAP, 0, 0, &[])?;
        }
        let out = work + 0x40;
        call(
            &mut self.exec,
            5,
            0x186ab0,
            0,
            0,
            &[s.player.raw(), s.dt.to_bits(), s.dt.to_bits(), out],
        )?;
        let read_started = Instant::now();
        let m = self.exec.runtime().memory();
        let action = m.read(out)?;
        self.history.insert(
            idx,
            History {
                raw: m.read(RAW)?,
                control: m.read(ctrl)?,
                layout: m.read(0x51ea88)?,
                latch: m.u8(0x51ebcc)?,
            },
        );
        add(&READ_NS, elapsed_ns(read_started));
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::DatumHandle;
    use runtime::{Dispatch, Guest, guest::Permissions};

    #[test]
    fn private_prefetch_skips_io_and_a_refused_wait_discards_only_the_sample() {
        for path in [Path::Alias, Path::View] {
            private_prefetch_on(path);
        }
    }

    fn private_prefetch_on(path: Path) {
        const WORK: u32 = 0x560000;
        const PLAYER: u32 = 0x550800;
        let mut g = Guest::new();
        g.map_zeroed(0x120000, 0x100000, Permissions::RX).unwrap();
        g.map_zeroed(0x460000, 0x110000, Permissions::RW).unwrap();
        g.set_u32(0x4e8c20, 0x550000).unwrap();
        g.set_u32(0x4ed284, 0x550100).unwrap();
        let mut d = Dispatch::default();
        for address in [0x1872d0, 0x190c34, 0x1557c0, 0x124c00, 0x217c40] {
            d.register_generated_rust(address, |cpu, ctx, _| ctx.memory().ret(cpu, 0))
                .unwrap();
        }
        d.register_generated_rust(ANIMATION_RESOURCE_REQUEST, |_, _, _| {
            panic!("animation prediction reached the I/O queue")
        })
        .unwrap();
        d.register_generated_rust(0x186ab0, |cpu, ctx, _| {
            let mut m = ctx.memory();
            let next = m.u8(RAW)? + 1;
            m.set_u8(RAW, next)?;
            let out = m.u32(cpu.esp + 16)?;
            m.write(out, [next; ACTION_SIZE])?;
            let ordinal = m.u32(WORK)?;
            if ordinal != 0 {
                return m.call(cpu, runtime::kernel::function_token(ordinal), 0x2d6307);
            }
            Ok(runtime::TransferAbi::jump(ANIMATION_RESOURCE_REQUEST))
        })
        .unwrap();
        let main = Runtime::new(g, d);
        let mut builder = Builder::new(&main, WORK, path, Stage::Guest);
        let cpu = CpuAbi {
            esp: WORK + 0xfff0,
            ..Default::default()
        };
        // A complete build, repeated refusals after the private writes, and a
        // good sample. A failed reset must also discard the old unit's history.
        for (tick, reset, ordinal, expected) in [
            (1, true, 0, Some(1)),
            (2, false, 234, None),
            (3, false, 234, None),
            (4, false, 0, Some(2)),
            (5, true, 234, None),
            (6, false, 0, Some(1)),
            (7, false, 128, None),
        ] {
            let sample = SeatInput {
                player: DatumHandle::new(21, 5),
                player_addr: PLAYER,
                unit: DatumHandle::new(if tick < 5 { 50 } else { 51 }, 9),
                tick,
                reset,
                dt: 1. / 30.,
                pad: input::State {
                    packet_number: ordinal,
                    ..Default::default()
                },
            };
            assert_eq!(
                builder.sample(&main, cpu, sample).unwrap(),
                expected.map(|n| [n; ACTION_SIZE]),
                "{path:?}, tick {tick}"
            );
            assert_eq!(main.memory().u8(RAW).unwrap(), 0);
            assert_eq!(
                main.memory().read::<ACTION_SIZE>(WORK + 0x40).unwrap(),
                [0; ACTION_SIZE]
            );
        }
        assert_eq!(
            builder.reported,
            BTreeSet::from([
                BuildFailure::Refused(runtime::kernel::function_token(234)),
                BuildFailure::Refused(runtime::kernel::function_token(128)),
            ])
        );
    }

    // Stand-ins at the traced entries check the register/stack contract. The
    // retail code itself comes from the coordinator's compiled game image.
    #[test]
    fn retail_input_binds_the_requested_seat_and_keeps_its_history_private() {
        // The Rust input stage refuses here (the fixture maps none of the
        // image's constants), so every piece runs its stand-in. The Rust
        // body declines at the builder's entry (its first read faults), so
        // the builder's stand-in answers with the body on too.
        for path in [Path::Alias, Path::View] {
            for stage in [Stage::Guest, Stage::Rust] {
                for with_body in [false, true] {
                    binds_the_requested_seat_on(path, stage, with_body);
                }
            }
        }
    }

    fn binds_the_requested_seat_on(path: Path, stage: Stage, with_body: bool) {
        const USERS: u32 = 0x550000;
        const CONTROLS: u32 = USERS + 0x100;
        const PLAYER: u32 = USERS + 0x400;
        const WORK: u32 = 0x560000;
        let mut g = Guest::new();
        g.map_zeroed(0x120000, 0x100000, Permissions::RX).unwrap();
        g.map_zeroed(0x460000, 0x110000, Permissions::RW).unwrap();
        g.set_u32(0x4e8c20, USERS).unwrap();
        g.set_u32(0x4ed284, CONTROLS).unwrap();
        g.set_u32(USERS + 12, 0x1234).unwrap();
        g.set_u32(PLAYER + 0x24, u32::MAX).unwrap();
        g.set_u32(
            bots::guest::slot(0) + bots::guest::SLOT_LOCAL_USER,
            u32::MAX,
        )
        .unwrap();
        let target = CONTROLS + 0x14 + crate::touch_glue::PC_TARGET;
        let strength = CONTROLS + 0x14 + crate::touch_glue::PC_CONE;
        g.set_u32(target, 0xdead0002).unwrap();
        g.set_u32(strength, 0.875f32.to_bits()).unwrap();
        let player = DatumHandle::new(21, 5);
        let unit = DatumHandle::new(50, 9);
        let mut d = Dispatch::default();
        for address in [0x1872d0, 0x190c34, 0x1557c0, 0x124c00, 0x217c40, 0x186ab0] {
            d.register_generated_rust(address, move |cpu, ctx, _| {
                let mut m = ctx.memory();
                match address {
                    0x1872d0 => {
                        assert_eq!((cpu.eax, cpu.ecx), (0, unit.raw()));
                        if m.u32(CONTROLS + 0x14)? == u32::MAX {
                            assert_eq!(m.u32(target)?, u32::MAX);
                            assert_eq!(m.u32(strength)?, 0);
                        }
                        m.set_u32(CONTROLS + 0x14, cpu.ecx)?;
                    }
                    0x1557c0 => {
                        assert_eq!(cpu.eax, unit.raw());
                        m.set_u32(OBSERVER, 0x11223344)?;
                        cpu.eax = 1;
                    }
                    0x124c00 => {
                        assert!((33..=34).contains(&m.u32(cpu.esp + 4)?));
                        let history = m.u8(RAW)? + 1;
                        m.set_u8(RAW, history)?;
                    }
                    0x186ab0 => {
                        assert_eq!(m.u32(cpu.esp + 4)?, player.raw());
                        assert_eq!(m.u32(USERS + 12)?, player.raw());
                        assert_eq!(m.u32(USERS + 0x1c)?, player.raw());
                        assert_eq!(m.u32(PLAYER + 0x24)?, 0);
                        assert_eq!(m.u32(bots::guest::slot(0))?, bots::guest::SLOT_PROFILE);
                        assert_eq!(
                            m.u32(bots::guest::slot(0) + bots::guest::SLOT_LOCAL_USER)?,
                            0
                        );
                        assert_eq!(m.u32(CONTROLS + 0x14)?, unit.raw());
                        assert_eq!(m.u32(DIRECTOR + 12)?, 0x23cbb0);
                        assert_eq!(m.u32(OBSERVER)?, 0x11223344);
                        let out = m.u32(cpu.esp + 16)?;
                        let history = m.u8(RAW)?;
                        // This seat acquired a target on the previous sample.
                        // Its retained assist is independent of the host's.
                        if history > 1 {
                            assert_eq!(m.u32(target)?, 0xe0010004);
                            assert_eq!(m.u32(strength)?, 0.5f32.to_bits());
                        }
                        m.set_u32(target, 0xe0010004)?;
                        m.set_u32(strength, 0.5f32.to_bits())?;
                        m.write(out, [history; ACTION_SIZE])?;
                    }
                    _ => (),
                }
                m.ret(cpu, 0)
            })
            .unwrap();
        }
        let main = Runtime::new(g, d);
        let mut builder = Builder::new(&main, WORK, path, stage).with_body(&main, with_body);
        let declined = body::DECLINED.load(Ordering::Relaxed);
        for (tick, reset, count) in [(1, true, 1), (2, false, 2), (9, true, 1)] {
            let seat = SeatInput {
                player,
                player_addr: PLAYER,
                unit,
                tick,
                dt: 1. / 30.,
                pad: Default::default(),
                reset,
            };
            assert_eq!(
                builder
                    .action(
                        &main,
                        CpuAbi {
                            esp: WORK + 0xffff - 15,
                            ..Default::default()
                        },
                        seat
                    )
                    .unwrap(),
                [count; ACTION_SIZE]
            );
        }
        let m = main.memory();
        assert_eq!(m.u32(PLAYER + 0x24).unwrap(), u32::MAX);
        assert_eq!(m.u32(USERS + 12).unwrap(), 0x1234);
        assert_eq!(m.u8(RAW).unwrap(), 0);
        assert_eq!(m.u32(target).unwrap(), 0xdead0002);
        assert_eq!(m.u32(strength).unwrap(), 0.875f32.to_bits());
        assert_eq!(m.u32(bots::guest::slot(0)).unwrap(), 0);
        assert_eq!(
            m.u32(bots::guest::slot(0) + bots::guest::SLOT_LOCAL_USER)
                .unwrap(),
            u32::MAX
        );
        let refused = (stage == Stage::Rust).then_some(Refusal::Memory);
        assert_eq!(builder.refused.first().copied(), refused, "{path:?}, {stage:?}");
        if with_body {
            assert!(body::DECLINED.load(Ordering::Relaxed) >= declined + 3, "{path:?}, {stage:?}");
        }
    }
}

/// The paths and the switch.
#[cfg(test)]
mod path_tests {
    use super::*;
    use guest_layout::DatumHandle;
    use runtime::{Dispatch, guest::Permissions};
    use std::sync::Arc;

    const USERS: u32 = 0x550000;
    const CONTROLS: u32 = USERS + 0x100;
    const WORK: u32 = 0x560000;
    const ESP: u32 = WORK + 0xfff0;
    /// An uninitialised local of the builder, below the gather frame: the
    /// live game's own calls between two gathers leave other bytes there.
    const LOCAL: u32 = ESP - 0x200;
    /// A scratch global the builder writes, as the searches do.
    const SCRATCH: u32 = 0x4e0000;

    /// The tables the setup writes, stand-ins that return at once for the
    /// first five entries, and a builder that reports SCRATCH and LOCAL in
    /// its action, then bumps SCRATCH.
    fn runtime() -> Arc<Runtime> {
        let mut g = Guest::new();
        g.map_zeroed(0x120000, 0x100000, Permissions::RX).unwrap();
        g.map_zeroed(0x460000, 0x110000, Permissions::RW).unwrap();
        g.set_u32(0x4e8c20, USERS).unwrap();
        g.set_u32(0x4ed284, CONTROLS).unwrap();
        g.set_u32(LOCAL, 0x1111).unwrap();
        let mut d = Dispatch::default();
        for address in [0x1872d0, 0x190c34, 0x1557c0, 0x124c00, 0x217c40] {
            d.register_generated_rust(address, |cpu, ctx, _| ctx.memory().ret(cpu, 0))
                .unwrap();
        }
        d.register_generated_rust(0x186ab0, |cpu, ctx, _| {
            let mut m = ctx.memory();
            let out = m.u32(cpu.esp + 16)?;
            let (scratch, local) = (m.u32(SCRATCH)?, m.u32(LOCAL)?);
            m.set_u32(SCRATCH, scratch + 1)?;
            let mut action = [0; ACTION_SIZE];
            action[..4].copy_from_slice(&scratch.to_le_bytes());
            action[4..8].copy_from_slice(&local.to_le_bytes());
            m.write(out, action)?;
            m.ret(cpu, 0)
        })
        .unwrap();
        Runtime::new(g, d)
    }

    fn seat(index: u16, tick: u32) -> SeatInput {
        SeatInput {
            player: DatumHandle::new(5, u32::from(index)),
            player_addr: USERS + 0x400 + 0x100 * u32::from(index - 20),
            unit: DatumHandle::new(9, u32::from(50 + index)),
            tick,
            dt: 1. / 30.,
            pad: Default::default(),
            reset: false,
        }
    }

    fn reported(action: [u8; ACTION_SIZE]) -> (u32, u32) {
        let word = |i: usize| u32::from_le_bytes(action[i..i + 4].try_into().unwrap());
        (word(0), word(4))
    }

    fn cpu() -> CpuAbi {
        CpuAbi {
            esp: ESP,
            ..Default::default()
        }
    }

    /// The view shows every seat of a tick the RAM of the tick's first
    /// gather. The alias undoes each seat before the next, and puts the
    /// first gather's stack window back, so a later seat sees neither an
    /// earlier seat's writes nor the stack bytes the live game left between
    /// the gathers.
    #[test]
    fn later_seats_of_a_tick_see_the_first_gathers_world_on_both_paths() {
        for path in [Path::Alias, Path::View] {
            let main = runtime();
            let mut builder = Builder::new(&main, WORK, path, Stage::Guest);
            let first = builder.action(&main, cpu(), seat(21, 1)).unwrap();
            assert_eq!(reported(first), (0, 0x1111), "{path:?}");
            // Between the gathers the live game's own calls run below the
            // gather frame.
            main.memory().set_u32(LOCAL, 0x2222).unwrap();
            let second = builder.action(&main, cpu(), seat(22, 1)).unwrap();
            assert_eq!(
                reported(second),
                (0, 0x1111),
                "{path:?}: no earlier seat's write, the first gather's stack"
            );
            // The next tick's first seat sees live RAM as it is.
            let next = builder.action(&main, cpu(), seat(21, 2)).unwrap();
            assert_eq!(reported(next), (0, 0x2222), "{path:?}");
            let m = main.memory();
            assert_eq!(
                (m.u32(SCRATCH), m.u32(LOCAL)),
                (Ok(0), Ok(0x2222)),
                "{path:?}: live RAM kept"
            );
        }
    }

    #[test]
    fn the_switch_picks_the_alias_by_default_and_the_view_when_off() {
        let parse = |spec: &str| path_for(&hooks::Switches::parse(spec).unwrap());
        assert_eq!(parse(""), Path::Alias);
        assert_eq!(parse("-impl.sim.phantom-alias"), Path::View);
        assert_eq!(parse("impl.sim.phantom-alias=off"), Path::View);
        assert_eq!(parse("-impl.sim"), Path::View, "through its parent");
        // The default builds on live RAM and never snapshots; off, it does.
        for (spec, snapshots) in [("", false), ("-impl.sim.phantom-alias", true)] {
            let main = runtime();
            let mut builder = Builder::new(&main, WORK, parse(spec), Stage::Guest);
            assert_eq!(builder.path, parse(spec));
            builder.action(&main, cpu(), seat(21, 1)).unwrap();
            assert_eq!(builder.exec.base().is_some(), snapshots, "{spec:?}");
            assert!(!builder.exec.aliased(), "{spec:?}: the alias closed");
        }
    }

    /// Check mode compares the two paths seat by seat and counts nothing
    /// when they agree; the switch's path gives the answer.
    #[test]
    fn check_mode_builds_both_paths_and_they_agree() {
        let main = runtime();
        let mut builder = Builder::new(&main, WORK, Path::Alias, Stage::Guest);
        builder.other = Some(Box::new(Builder::on(&main, WORK, Path::View, Stage::Guest)));
        let checked = CHECKED.load(Ordering::Relaxed);
        let mismatches = MISMATCHES.load(Ordering::Relaxed);
        for (index, tick) in [(21, 1), (22, 1), (21, 2), (22, 2)] {
            let action = builder.sample_checked(&main, cpu(), seat(index, tick)).unwrap();
            assert_eq!(action.map(reported), Some((0, 0x1111)));
        }
        // Other tests may count at the same time: at least these four.
        assert!(CHECKED.load(Ordering::Relaxed) >= checked + 4);
        assert_eq!(MISMATCHES.load(Ordering::Relaxed), mismatches);
        assert!(builder.other.as_ref().unwrap().exec.base().is_some(), "the view arm ran");
        assert_eq!(main.memory().u32(SCRATCH), Ok(0));
    }
}

/// Image-backed counterpart to the synthetic dispatch and transport tests.
/// A local multiplayer checkpoint supplies real player, BSP and weapon tags.
#[cfg(test)]
#[test]
#[ignore = "requires a built retail module and a local multiplayer checkpoint with an opponent under the reticle"]
fn retail_pad_sequence_matches_local_angles_target_and_fire() -> Result<()> {
    use guest_layout::DatumHandle;
    use runtime::guest::Permissions;
    use std::path::PathBuf;

    let file = |key| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("missing {key}"))
    };
    let xbe = crate::read_xbe(&file("H2_DISC")?)?;
    let module = file("H2_RETAIL_MODULE")?;
    let bytes = std::fs::read(file("H2_PAD_CHECKPOINT")?)?;
    let header = snapshot::read_header(&bytes)?;
    let saved = snapshot::Snapshot::open(&bytes, &header.build)?;
    let mut loaded = runtime::load_xbe_with(&xbe, &runtime::loader::LoadOptions::halo2())?;
    let kernel = kernel::Kernel::for_image(&mut loaded)?;
    let main = &loaded.runtime;
    // SAFETY: H2_RETAIL_MODULE is the coordinator's generated module for this XBE.
    let _module = unsafe { crate::load_module(&module, main, &xbe)? };
    // Both paths, and the alias with each input stage, each from the
    // checkpoint as saved.
    for (path, stage) in [
        (Path::Alias, Stage::Rust),
        (Path::Alias, Stage::Guest),
        (Path::View, Stage::Guest),
    ] {
        snapshot::guest::load_guest(&saved, &mut main.memory(), None)?;
        const WORK: u32 = 0xa3fe0000;
        let (player, player_addr, unit) = {
            let mut m = main.memory();
            // Mapped by the first path's pass when the checkpoint's layout
            // load leaves it.
            match m.map_zeroed(WORK, 0x10000, Permissions::RW) {
                Ok(()) | Err(runtime::guest::MapError::Overlap) => {}
                Err(e) => return Err(e.into()),
            }
            let user = m.u32(0x4e8c20)?;
            let player = DatumHandle(m.u32(user + 12)?);
            anyhow::ensure!(!player.is_none(), "fixture needs local user zero");
            let array = m.u32(0x4e8c24)?;
            let addr = m.u32(array + 0x44)? + 0x21c * u32::from(player.index());
            let unit = DatumHandle(m.u32(addr + 0x2c)?);
            anyhow::ensure!(
                m.u32(addr + 0x24)? == 0 && !unit.is_none(),
                "fixture needs a spawned player on controller zero"
            );
            m.write(RAW, [0; 0x48])?;
            m.write(0x51ea88, [0; 0x50])?;
            m.set_u8(0x51ebcc, 0)?;
            let controls = m.u32(0x4ed284)?;
            m.set_u32(controls + 0x14, u32::MAX)?;
            m.write(0x54e9f4, [0, 0, 0, 0, 0, 0, bots::guest::BOT_SENSITIVITY])?;
            (player, addr, unit)
        };
        let cpu = CpuAbi {
            esp: WORK + 0xfff0,
            fs_base: kernel.kpcr(),
            ..CpuAbi::xbox_thread()
        };
        let mut local = SecondExecutor::new(main, |_, _| false);
        local.runtime().edit_dispatch(|d| {
            d.prefer_generated();
            d.register_override(
                xapi::register::addr::XINPUT_GET_STATE,
                "fixture controller",
                |cpu, ctx, _| {
                    let mut m = ctx.memory();
                    let report = m.read::<{ input::State::SIZE }>(WORK)?;
                    let dest = m.u32(cpu.esp + 8)?;
                    m.write(dest, report)?;
                    cpu.eax = 0;
                    m.ret(cpu, 8)
                },
            );
        });
        local.snapshot(main);
        // The reference retains the game's actual local tables. The other side
        // must bind this same world actor starting from a remote player record.
        {
            let mut m = main.memory();
            m.set_u32(player_addr + 0x24, u32::MAX)?;
            m.set_u16(player_addr + 0x28, u16::MAX)?;
            let slot = bots::guest::slot(0);
            m.set_u32(slot, 0)?;
            m.set_u32(slot + bots::guest::SLOT_LOCAL_USER, u32::MAX)?;
            let users = m.u32(0x4e8c20)?;
            m.set_u32(users + 12, u32::MAX)?;
            m.set_u32(users + 0x1c, u32::MAX)?;
        }
        let mut remote = Builder::new(main, WORK, path, stage);
        let dt = 1.0f32 / 30.0;
        let mut acquired = false;
        let mut fired = false;
        let mut angles = Vec::new();
        for tick in 0..30 {
            let mut pad = input::State {
                packet_number: tick,
                ..Default::default()
            };
            pad.gamepad.thumb_rx = if tick < 10 {
                0
            } else if tick < 22 {
                19000
            } else {
                -22000
            };
            pad.gamepad.thumb_ry = if tick < 10 { 0 } else { 11000 };
            pad.gamepad.analog[input::AnalogButton::RightTrigger as usize] =
                if tick % 6 < 3 { 255 } else { 0 };
            local.runtime().memory().write(WORK, pad.to_bytes())?;
            let mut call = |pc, eax, ecx, args: &[u32]| -> Result<()> {
                local.call(&mut CpuAbi { eax, ecx, ..cpu }, pc, args)?;
                Ok(())
            };
            if tick == 0 {
                call(0x1872d0, 0, unit.raw(), &[])?;
                call(0x190c34, 0, 0, &[])?;
            }
            call(0x124c00, 0, 0, &[if tick % 3 == 1 { 34 } else { 33 }])?;
            call(0x217c40, 0, 0, &[])?;
            call(
                0x186ab0,
                0,
                0,
                &[player.raw(), dt.to_bits(), dt.to_bits(), WORK + 0x40],
            )?;
            let expected = local.runtime().memory().read::<ACTION_SIZE>(WORK + 0x40)?;
            let action = remote.action(
                main,
                cpu,
                SeatInput {
                    player,
                    player_addr,
                    unit,
                    tick,
                    dt,
                    pad,
                    reset: tick == 0,
                },
            )?;
            assert_eq!(action, expected, "{path:?}, {stage:?} input: retail action at sample {tick}");
            let word = |off: usize| u32::from_le_bytes(action[off..off + 4].try_into().unwrap());
            acquired |= word(0x34) != u32::MAX
                && word(0x3c) != u32::MAX
                && (f32::from_bits(word(0x50)) > 0. || f32::from_bits(word(0x54)) > 0.);
            fired |= action[0] & 0x40 != 0;
            angles.push(action[4..12].to_vec());
        }
        assert!(fired && angles.windows(2).any(|p| p[0] != p[1]));
        assert!(
            acquired,
            "fixture needs a visible opposing player within weapon assist range"
        );
    }
    Ok(())
}

/// The Rust input stage inside a build: the guest entries are not called,
/// the builder reads what the stage wrote, and live RAM is untouched.
#[cfg(test)]
mod stage_tests {
    use super::*;
    use guest_layout::DatumHandle;
    use runtime::{Dispatch, guest::Permissions};

    const USERS: u32 = 0x550000;
    const CONTROLS: u32 = USERS + 0x100;
    const PLAYER: u32 = USERS + 0x400;
    const HEADER: u32 = USERS + 0x800;
    const TABLE: u32 = USERS + 0x900;
    const OBJECT: u32 = USERS + 0xa00;
    const WORK: u32 = 0x560000;

    /// The image's constants the stage reads (their retail values), the
    /// tables the setup writes, and one unit facing +y.
    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x120000, 0x100000, Permissions::RX).unwrap();
        g.map_zeroed(0x430000, 0x140000, Permissions::RW).unwrap();
        g.write(0x440c38, [0u8, 1, 2, 3, 4, 5, 6, 7]).unwrap();
        g.write(0x440c40, [1u8, 2, 4, 8, 0x10, 0x20, 0x40, 0x80]).unwrap();
        for i in 0..10u32 {
            g.set_u32(0x444bdc + 4 * i, (40. + 10. * i as f32).to_bits()).unwrap();
            g.set_u32(0x444c04 + 4 * i, (80. + 20. * i as f32).to_bits()).unwrap();
        }
        let pi = std::f32::consts::PI;
        for (at, v) in [
            (0x43ba70, 1. / 32767.),
            (0x43f17c, 2. * pi),
            (0x44a038, 120.),
            (0x44a03c, 60.),
            (0x45dbb4, 1. / 255.),
            (0x45dbc0, 1.),
            (0x45dbcc, -1.),
            (0x45e010, 0.174_532_92),
            (0x45e014, 1.637_022_3),
            (0x45e018, 3. * pi / 4.),
            (0x45e01c, pi / 4.),
            (0x45e020, 0.610_865_2),
            (0x45e050, -1.492_256_5),
            (0x45e054, 1.492_256_5),
            (0x4570a0, pi / 4.),
            (0x4570a4, 3. * pi / 4.),
            (0x4570a8, -pi / 4.),
            (0x4570ac, -3. * pi / 4.),
        ] {
            g.set_u32(at, f32::to_bits(v)).unwrap();
        }
        g.set_u32(0x4e8c20, USERS).unwrap();
        g.set_u32(0x4ed284, CONTROLS).unwrap();
        g.set_u8(CONTROLS, 1).unwrap();
        g.set_u32(0x4e0300, HEADER).unwrap();
        g.set_u32(HEADER + 0x44, TABLE).unwrap();
        g.set_u32(TABLE + 50 * 12 + 8, OBJECT).unwrap();
        g.set_u32(OBJECT + 0x154, 1f32.to_bits()).unwrap();
        g
    }

    #[test]
    fn the_rust_stage_runs_no_input_entry_and_leaves_live_ram() {
        let player = DatumHandle::new(21, 5);
        let unit = DatumHandle::new(9, 50);
        for path in [Path::Alias, Path::View] {
            let mut d = Dispatch::default();
            for address in [0x1872d0, 0x190c34, 0x124c00, 0x217c40] {
                d.register_generated_rust(address, move |_, _, _| {
                    panic!("{path:?}: the guest entry {address:#x} ran")
                })
                .unwrap();
            }
            d.register_generated_rust(0x1557c0, |cpu, ctx, _| ctx.memory().ret(cpu, 0))
                .unwrap();
            // The builder reports the abstract input, the look rates' mode
            // word and the bound control block's unit and yaw.
            d.register_generated_rust(0x186ab0, |cpu, ctx, _| {
                let mut m = ctx.memory();
                let out = m.u32(cpu.esp + 16)?;
                let mut action = [0; ACTION_SIZE];
                action[..0x50].copy_from_slice(&m.read::<0x50>(0x51ea88)?);
                action[0x50..0x54].copy_from_slice(&m.read::<4>(CONTROLS + 0x14)?);
                action[0x54..0x58].copy_from_slice(&m.read::<4>(CONTROLS + 0x24)?);
                action[0x58] = m.u8(0x51ea18 + 0x18)?;
                m.write(out, action)?;
                m.ret(cpu, 0)
            })
            .unwrap();
            let main = Runtime::new(guest(), d);
            let before = main.memory().read::<0x48>(RAW).unwrap();
            let mut builder = Builder::new(&main, WORK, path, Stage::Rust);
            let cpu = CpuAbi {
                esp: WORK + 0xfff0,
                ..Default::default()
            };
            let mut pad = input::State::default();
            pad.gamepad.thumb_ly = i16::MAX;
            pad.gamepad.analog[0] = 255;
            let mut actions = Vec::new();
            for tick in 1..=2 {
                let seat = SeatInput {
                    player,
                    player_addr: PLAYER,
                    unit,
                    tick,
                    dt: 1. / 30.,
                    pad,
                    reset: tick == 1,
                };
                actions.push(builder.action(&main, cpu, seat).unwrap());
            }
            assert!(builder.refused.is_empty(), "{path:?}: {:?}", builder.refused);
            let word = |a: &[u8; ACTION_SIZE], at: usize| u32::from_le_bytes(a[at..at + 4].try_into().unwrap());
            let [first, second] = [actions[0], actions[1]];
            // Bound on the first build, facing +y: a yaw of pi/2.
            assert_eq!(word(&first, 0x50), unit.raw(), "{path:?}");
            assert_eq!(f32::from_bits(word(&first, 0x54)), std::f32::consts::FRAC_PI_2);
            // Full up on the left stick is +1 on 0x38; the right stick is centred.
            assert_eq!(f32::from_bits(word(&second, 0x38)), 1., "{path:?}");
            assert_eq!(word(&second, 0x3c), 0f32.to_bits());
            assert_eq!(first[0x58], 0, "the Default layout's mode");
            // The analog button is held from the second read: one down tick.
            assert_eq!((first[0], second[0]), (0, 1), "{path:?}");
            let m = main.memory();
            assert_eq!(m.read::<0x48>(RAW).unwrap(), before, "{path:?}: live raw input kept");
            assert_eq!(m.u32(CONTROLS + 0x14).unwrap(), 0, "{path:?}: live control kept");
        }
    }
}
