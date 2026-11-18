//! The telemetry worker (docs/live/live-telemetry.md, "Threads"): the game
//! thread only reads the guest into plain observations and queues them;
//! one worker thread runs the [`Session`] over them in order, builds the
//! events and frames (strings, sorting, tag and team names) and hands them
//! to the [`Sink`].
//!
//! The queue keeps every start, kill, end and setting. Ticks are bounded:
//! past [`MAX_QUEUED_TICKS`] waiting, a new tick is folded into the last
//! queued one ([`fold`]) instead of queued. The worker hands each tick's
//! snapshot back cleared, so a steady game thread reuses their allocations.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use live_proto::game::TeamScore;

use crate::event::{GameInfo, ObjectState, Output, Placements, PlayerState, Snapshot};
use crate::session::Session;
use crate::sink::Sink;
use crate::tags::{MapFile, TagNames};

/// Ticks waiting before a new one is folded into the last.
pub(crate) const MAX_QUEUED_TICKS: usize = 4;
/// Everything waiting; past this a start, kill or end fails (and telemetry
/// stops) rather than be dropped.
pub(crate) const MAX_QUEUED: usize = 256;
/// Damage reports a folded tick keeps (oldest dropped past it), as the
/// damage hook keeps between ticks.
pub(crate) const MAX_FOLDED_HITS: usize = 256;
/// How often a worker in a game looks for ticks. Ticks do not wake it: on
/// the game thread a wake is a system call (a futex wake on Linux), so in a
/// game the worker polls instead, and a tick waits at most this long. One
/// game tick.
const POLL: std::time::Duration = std::time::Duration::from_millis(33);
/// A worker with no work for this long parks (waits with no deadline)
/// until the next work wakes it, so a menu or a paused game costs nothing.
const PARK_AFTER: std::time::Duration = std::time::Duration::from_secs(1);
/// Cleared snapshots kept for the game thread to reuse.
const SPARE_SNAPSHOTS: usize = 4;

/// A match start as the game thread read it.
pub(crate) struct Start {
    pub info: GameInfo,
    pub players: Vec<PlayerState>,
    pub teams: Vec<TeamScore>,
    pub map_id: Option<live_proto::maps::MapId>,
    pub placements: Placements,
    /// The map's file and the guest's scenario datum: the worker loads the
    /// tag names and checks they are the loaded map's.
    pub names: Option<Names>,
}

pub(crate) struct Names {
    pub map: String,
    pub file: MapFile,
    pub scenario: Option<u32>,
}

impl Names {
    fn load(self) -> Option<TagNames> {
        let names = match TagNames::load(&self.file) {
            Ok(names) => names,
            Err(error) => {
                eprintln!("live telemetry: no tag names for {}: {error}", self.map);
                return None;
            }
        };
        // An unreadable tag index: no names, and nothing to say.
        if !names.is_for(self.scenario?) {
            eprintln!(
                "live telemetry: {} is not the loaded {}",
                self.file.path.display(),
                self.map
            );
            return None;
        }
        Some(names)
    }
}

pub(crate) enum Work {
    /// A setting for the matches to come, in order with them.
    Configure(Box<dyn FnOnce(&mut Session) + Send>),
    Start(Box<Start>),
    Tick(Box<Snapshot>),
    Kill(Box<Kill>),
    End(Box<Snapshot>),
}

pub(crate) struct Kill {
    pub tick: u64,
    pub attacker: Option<u32>,
    pub victim: u32,
    pub betrayal: bool,
    pub object: Option<ObjectState>,
}

#[derive(Default)]
struct Queue {
    work: VecDeque<Work>,
    ticks: usize,
    /// The worker holds a batch.
    busy: bool,
    /// The worker waits with no deadline: only a wake gets it going.
    parked: bool,
    stop: bool,
}

/// Whether queuing `work` wakes the worker: a start, kill, end or setting
/// always does (they are rare); a tick only a parked worker, which would
/// otherwise not look. A worker in a game polls ([`POLL`]).
fn wakes(work: &Work, parked: bool) -> bool {
    parked || !matches!(work, Work::Tick(_))
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    /// Work queued or stop asked.
    wake: Condvar,
    /// The queue ran empty and the worker is between batches.
    idle: Condvar,
    #[allow(clippy::vec_box, reason = "each goes back into a Work::Tick unmoved")]
    spare: Mutex<Vec<Box<Snapshot>>>,
    error: Mutex<Option<String>>,
    /// `error` is set: the game thread checks this, not the lock.
    failed: AtomicBool,
    folded: AtomicU64,
    /// Time the worker spent on work, for the bench.
    work_ns: AtomicU64,
}

impl Shared {
    fn recycle(&self, mut snapshot: Box<Snapshot>) {
        // The roster stays: its names are the next read's.
        snapshot.teams.clear();
        snapshot.objects.clear();
        snapshot.shots.clear();
        snapshot.actors.clear();
        snapshot.vehicles.clear();
        snapshot.machines.clear();
        snapshot.placed.clear();
        snapshot.bodies.clear();
        snapshot.hits.clear();
        let mut spare = self.spare.lock().unwrap();
        if spare.len() < SPARE_SNAPSHOTS {
            spare.push(snapshot);
        }
    }

    fn fail(&self, error: String) {
        self.error.lock().unwrap().get_or_insert(error);
        self.failed.store(true, Ordering::Release);
    }
}

/// Folds `newer` into `queued` (the last queued tick) and returns the
/// snapshot left over. The later tick is kept, unless only the earlier is
/// a frame tick (even): then the frame is kept and the later tick, whose
/// roster and scores the next tick reads again, goes. Either way the one
/// kept takes the other's shots (added) and hits (in tick order).
pub(crate) fn fold(queued: &mut Box<Snapshot>, mut newer: Box<Snapshot>) -> Box<Snapshot> {
    let frame = |s: &Snapshot| s.tick.is_multiple_of(2);
    let keep_queued = frame(queued) && !frame(&newer);
    if !keep_queued {
        std::mem::swap(queued, &mut newer);
    }
    let gone = &mut newer;
    for (weapon, n) in &gone.shots {
        queued.shots.add(*weapon, *n);
    }
    if keep_queued {
        queued.hits.append(&mut gone.hits);
    } else {
        gone.hits.append(&mut queued.hits);
        std::mem::swap(&mut gone.hits, &mut queued.hits);
    }
    let over = queued.hits.len().saturating_sub(MAX_FOLDED_HITS);
    queued.hits.drain(..over);
    newer
}

pub struct Pipeline {
    shared: Arc<Shared>,
    #[cfg_attr(not(test), allow(dead_code, reason = "tests read it once settled"))]
    session: Arc<Mutex<Session>>,
    logged: AtomicBool,
    worker: Option<JoinHandle<()>>,
}

impl Pipeline {
    pub fn new<S: Sink>(session: Session, mut sink: S) -> Self {
        let shared = Arc::new(Shared::default());
        let session = Arc::new(Mutex::new(session));
        let worker = {
            let shared = Arc::clone(&shared);
            let session = Arc::clone(&session);
            std::thread::Builder::new()
                .name("h2-telemetry".into())
                .spawn(move || run(&shared, &session, &mut sink))
                .expect("spawn telemetry worker")
        };
        Self {
            shared,
            session,
            logged: AtomicBool::new(false),
            worker: Some(worker),
        }
    }

    /// Queues `work` for the worker. A tick past [`MAX_QUEUED_TICKS`] is
    /// folded into the last queued one when that is a tick; anything else
    /// past [`MAX_QUEUED`] is an error. A tick wakes only a parked worker
    /// ([`wakes`]), so a steady game's ticks make no system call.
    pub(crate) fn push(&self, work: Work) -> Result<(), String> {
        if let Some(error) = self.worker_error() {
            return Err(error);
        }
        let mut queue = self.shared.queue.lock().unwrap();
        let wake = wakes(&work, queue.parked);
        let full = queue.work.len() >= MAX_QUEUED;
        match work {
            Work::Tick(snapshot) => {
                let behind = full || queue.ticks >= MAX_QUEUED_TICKS;
                let left = match queue.work.back_mut() {
                    Some(Work::Tick(last)) if behind => Some(fold(last, snapshot)),
                    // Only with a start, kill or end last, past MAX_QUEUED:
                    // the tick goes.
                    _ if full => Some(snapshot),
                    _ => {
                        queue.ticks += 1;
                        queue.work.push_back(Work::Tick(snapshot));
                        None
                    }
                };
                if let Some(left) = left {
                    drop(queue);
                    self.shared.folded.fetch_add(1, Ordering::Relaxed);
                    self.shared.recycle(left);
                    return Ok(());
                }
            }
            _ if full => return Err("telemetry queue is full".into()),
            work => queue.work.push_back(work),
        }
        drop(queue);
        if wake {
            self.shared.wake.notify_one();
        }
        Ok(())
    }

    /// A cleared snapshot the worker is done with (its roster kept for the
    /// names), or a new one.
    pub(crate) fn snapshot(&self) -> Box<Snapshot> {
        self.shared.spare.lock().unwrap().pop().unwrap_or_default()
    }

    /// Gives back a snapshot that was not queued.
    pub(crate) fn recycle(&self, snapshot: Box<Snapshot>) {
        self.shared.recycle(snapshot);
    }

    /// Ticks folded into another because the worker was behind.
    pub fn folded_ticks(&self) -> u64 {
        self.shared.folded.load(Ordering::Relaxed)
    }

    /// Nanoseconds the worker has spent on work so far.
    #[cfg(test)]
    pub(crate) fn work_ns(&self) -> u64 {
        self.shared.work_ns.load(Ordering::Relaxed)
    }

    /// Waits until the worker has done everything queued.
    pub fn settle(&self) {
        self.shared.wake.notify_one();
        let mut queue = self.shared.queue.lock().unwrap();
        while !queue.work.is_empty() || queue.busy {
            queue = self.shared.idle.wait(queue).unwrap();
        }
    }

    /// The session, once the worker has done everything queued.
    #[cfg(test)]
    pub(crate) fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.settle();
        self.session.lock().unwrap()
    }

    pub fn worker_error(&self) -> Option<String> {
        // The game thread asks every tick: an atomic load while all is well.
        if !self.shared.failed.load(Ordering::Acquire) {
            return None;
        }
        self.shared.error.lock().unwrap().clone()
    }

    pub fn log_error(&self, reason: &str) {
        if !self.logged.swap(true, Ordering::AcqRel) {
            eprintln!("live telemetry disabled: {reason}");
        }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().stop = true;
        self.shared.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(error) = self.worker_error() {
            self.log_error(&error);
        }
    }
}

/// The worker: batches of work in queue order until stopped, then what is
/// left. After a sink error the rest is dropped.
fn run(shared: &Shared, session: &Mutex<Session>, sink: &mut dyn Sink) {
    let mut batch = VecDeque::new();
    let mut failed = false;
    // When the worker last had work; parked until the first.
    let mut active: Option<Instant> = None;
    loop {
        {
            let mut queue = shared.queue.lock().unwrap();
            queue.busy = false;
            while queue.work.is_empty() && !queue.stop {
                shared.idle.notify_all();
                if active.is_some_and(|at| at.elapsed() < PARK_AFTER) {
                    queue = shared.wake.wait_timeout(queue, POLL).unwrap().0;
                } else {
                    queue.parked = true;
                    queue = shared.wake.wait(queue).unwrap();
                    queue.parked = false;
                }
            }
            if queue.work.is_empty() {
                shared.idle.notify_all();
                break;
            }
            std::mem::swap(&mut queue.work, &mut batch);
            queue.ticks = 0;
            queue.busy = true;
        }
        let start = Instant::now();
        active = Some(start);
        let mut session = session.lock().unwrap();
        for work in batch.drain(..) {
            if failed {
                continue;
            }
            let done = apply(&mut session, work);
            for output in session.take_output() {
                let sent = match output {
                    Output::Event(event) => sink.event(event),
                    Output::Frame(frame) => sink.frame(frame),
                };
                if let Err(error) = sent {
                    failed = true;
                    shared.fail(error);
                    break;
                }
            }
            if let Some(snapshot) = done {
                shared.recycle(snapshot);
            }
        }
        drop(session);
        if !failed && let Err(error) = sink.flush() {
            failed = true;
            shared.fail(error);
        }
        shared
            .work_ns
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

/// One piece of work into the session; the snapshot it is done with.
fn apply(session: &mut Session, work: Work) -> Option<Box<Snapshot>> {
    match work {
        Work::Configure(f) => f(session),
        Work::Start(start) => {
            let Start {
                info,
                players,
                teams,
                map_id,
                placements,
                names,
            } = *start;
            session.set_map_id(map_id);
            session.start_with_teams(&info, players, teams);
            if let Some(names) = names.and_then(Names::load) {
                session.set_tag_names(names);
            }
            session.set_placements(placements);
        }
        Work::Tick(snapshot) => {
            session.sample(&snapshot);
            return Some(snapshot);
        }
        Work::Kill(kill) => {
            let Kill {
                tick,
                attacker,
                victim,
                betrayal,
                object,
            } = *kill;
            session.kill(tick, attacker, victim, betrayal, object);
        }
        Work::End(snapshot) => {
            session.end(&snapshot);
            return Some(snapshot);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{GameEvent, HitRecord};
    use live_proto::game::Frame;
    use std::sync::mpsc;

    fn tick(tick: u64, shots: &[(u32, u32)], hits: &[u32]) -> Box<Snapshot> {
        Box::new(Snapshot {
            tick,
            shots: shots.iter().copied().collect(),
            hits: hits.iter().map(|h| hit(*h)).collect(),
            ..Snapshot::default()
        })
    }

    fn hit(n: u32) -> HitRecord {
        HitRecord {
            code: 7,
            effect: 0,
            owner_player: u32::MAX,
            owner_object: 1,
            victim: n,
            pos: None,
            body: 0.0,
            shield: 0.0,
            tick: None,
        }
    }

    fn codes(s: &Snapshot) -> Vec<u32> {
        s.hits.iter().map(|h| h.victim).collect()
    }

    #[test]
    fn a_later_tick_folds_in_the_earlier_ones_shots_and_hits() {
        let mut queued = tick(3, &[(7, 1), (8, 2)], &[1, 2]);
        let left = fold(&mut queued, tick(4, &[(7, 4)], &[3]));
        assert_eq!(queued.tick, 4);
        assert_eq!(left.tick, 3);
        assert_eq!(queued.shots, [(7, 5), (8, 2)].into());
        assert_eq!(codes(&queued), [1, 2, 3], "in tick order");
        // Two frame ticks, or two others: the later is kept too.
        let mut queued = tick(4, &[], &[1]);
        assert_eq!(fold(&mut queued, tick(6, &[], &[2])).tick, 4);
        assert_eq!((queued.tick, codes(&queued)), (6, vec![1, 2]));
        let mut queued = tick(5, &[], &[]);
        assert_eq!(fold(&mut queued, tick(7, &[], &[])).tick, 5);
        assert_eq!(queued.tick, 7);
    }

    #[test]
    fn a_queued_frame_tick_outlasts_a_later_other_tick() {
        let mut queued = tick(4, &[(7, 1)], &[1]);
        let left = fold(&mut queued, tick(5, &[(7, 2), (9, 1)], &[2]));
        assert_eq!(queued.tick, 4, "the frame is kept");
        assert_eq!(left.tick, 5);
        assert_eq!(queued.shots, [(7, 3), (9, 1)].into());
        assert_eq!(codes(&queued), [1, 2]);
    }

    #[test]
    fn folded_hits_keep_the_newest() {
        let old: Vec<u32> = (0..200).collect();
        let new: Vec<u32> = (200..300).collect();
        let mut queued = tick(3, &[], &old);
        fold(&mut queued, tick(4, &[], &new));
        assert_eq!(queued.hits.len(), MAX_FOLDED_HITS);
        assert_eq!(codes(&queued)[0], 300 - MAX_FOLDED_HITS as u32);
        assert_eq!(*codes(&queued).last().unwrap(), 299);
    }

    #[test]
    fn a_tick_wakes_only_a_parked_worker() {
        // In a game the worker polls: a tick makes no system call.
        assert!(!wakes(&Work::Tick(tick(2, &[], &[])), false));
        assert!(!wakes(&Work::Tick(tick(3, &[], &[])), false));
        // After a pause or a menu it is parked: the first tick wakes it.
        assert!(wakes(&Work::Tick(tick(4, &[], &[])), true));
        // Kills, starts, ends and settings always wake it.
        assert!(wakes(&kill(5), false));
        assert!(wakes(&Work::Configure(Box::new(|_| {})), false));
    }

    /// Blocks in its first frame until released; records everything.
    struct Gate {
        seen: Arc<Mutex<Vec<String>>>,
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        blocked: bool,
    }

    impl Sink for Gate {
        fn event(&mut self, event: GameEvent) -> Result<(), String> {
            self.seen.lock().unwrap().push(event.kind);
            Ok(())
        }
        fn frame(&mut self, frame: Frame) -> Result<(), String> {
            self.seen
                .lock()
                .unwrap()
                .push(format!("frame {}", frame.tick));
            if !self.blocked {
                self.blocked = true;
                self.entered.send(()).unwrap();
                let _ = self.release.recv();
            }
            Ok(())
        }
    }

    fn started(session: &mut Session) {
        session.start(
            &GameInfo {
                map: "lockout".into(),
                mode: "multiplayer".into(),
                difficulty: None,
                skulls: None,
                variant: None,
                team_play: false,
                score_limit: None,
                time_limit_s: None,
                tick_rate: 30,
                tick: 1,
            },
            Vec::new(),
        );
    }

    fn kill(tick: u64) -> Work {
        Work::Kill(Box::new(Kill {
            tick,
            attacker: None,
            victim: 0,
            betrayal: false,
            object: None,
        }))
    }

    #[test]
    fn a_worker_behind_folds_ticks_but_keeps_every_kill_in_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (entered, wait) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let pipeline = Pipeline::new(
            Session::new("test", None),
            Gate {
                seen: Arc::clone(&seen),
                entered,
                release: gate,
                blocked: false,
            },
        );
        pipeline.push(Work::Configure(Box::new(started))).unwrap();
        pipeline.push(Work::Tick(tick(2, &[], &[]))).unwrap();
        wait.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the worker wakes on work");
        // The worker is held in frame 2: ticks 3.. queue, past four fold.
        for t in 3..12 {
            pipeline.push(Work::Tick(tick(t, &[], &[]))).unwrap();
        }
        pipeline.push(kill(12)).unwrap();
        pipeline.push(Work::Tick(tick(14, &[], &[]))).unwrap();
        assert_eq!(pipeline.folded_ticks(), 5);
        release.send(()).unwrap();
        pipeline.settle();
        let seen = seen.lock().unwrap().clone();
        // Ticks 3 to 6 queued; 7 to 11 folded into 6 (10 being the last
        // frame tick, it stays), then the kill, then frame 14.
        assert_eq!(
            seen,
            [
                "GameStarted",
                "frame 2",
                "frame 4",
                "frame 10",
                "PlayerKilled",
                "frame 14"
            ]
        );
        // What the worker is done with comes back cleared, for reuse.
        let spare = pipeline.snapshot();
        assert!(spare.hits.is_empty() && spare.actors.is_empty());
    }

    #[test]
    fn a_tick_waits_at_most_a_poll() {
        let pipeline = Pipeline::new(
            Session::new("test", None),
            Gate {
                seen: Arc::default(),
                entered: mpsc::channel().0,
                release: mpsc::channel().1,
                blocked: true,
            },
        );
        pipeline.push(Work::Configure(Box::new(started))).unwrap();
        pipeline.settle();
        pipeline.push(Work::Tick(tick(3, &[], &[]))).unwrap();
        // Nothing wakes the worker, and it still takes the tick.
        let deadline = Instant::now() + POLL * 60;
        while pipeline.shared.spare.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the tick waits");
            std::thread::sleep(POLL / 10);
        }
        assert_eq!(pipeline.session().current_tick(), Some(3));
    }

    #[test]
    fn a_full_queue_refuses_a_kill_rather_than_drop_it() {
        let (entered, wait) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let pipeline = Pipeline::new(
            Session::new("test", None),
            Gate {
                seen: Arc::default(),
                entered,
                release: gate,
                blocked: false,
            },
        );
        pipeline.push(Work::Configure(Box::new(started))).unwrap();
        pipeline.push(Work::Tick(tick(2, &[], &[]))).unwrap();
        wait.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        for t in 0..MAX_QUEUED as u64 {
            pipeline.push(kill(t)).unwrap();
        }
        assert_eq!(
            pipeline.push(kill(0)).unwrap_err(),
            "telemetry queue is full"
        );
        // A tick then goes (there is no tick last to fold it into).
        pipeline.push(Work::Tick(tick(4, &[], &[]))).unwrap();
        assert_eq!(pipeline.folded_ticks(), 1);
        release.send(()).unwrap();
    }
}
