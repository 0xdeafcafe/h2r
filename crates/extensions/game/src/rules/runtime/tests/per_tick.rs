//! What each tick of a running script costs the host: the `RULES` record
//! is copied only when something else changed it and written once when
//! the VM changed it.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// [`Local`], counting the record's copies (`get`) and writes (`set`).
#[derive(Clone, Default)]
struct Counting {
    local: Local,
    gets: Arc<AtomicUsize>,
    sets: Arc<AtomicUsize>,
}

impl Counting {
    fn gets(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }
    fn sets(&self) -> usize {
        self.sets.load(Ordering::SeqCst)
    }
}

impl Store for Counting {
    fn get(&self) -> Option<Vec<u8>> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.local.get()
    }
    fn same(&self, written: &[u8]) -> Option<bool> {
        self.local.same(written)
    }
    fn set(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        self.sets.fetch_add(1, Ordering::SeqCst);
        self.local.set(bytes)
    }
    fn remove(&mut self) {
        self.local.remove();
    }
}

/// Runs `f` on the runtime and checks what it did to the record: one write
/// when the bytes changed, none when they did not, and the record is the
/// VM's state after it.
fn step(s: &Arc<Shared>, store: &Counting, f: impl FnOnce(&mut Runtime)) {
    let (before, sets) = (store.local.bytes(), store.sets());
    f(&mut *s.lock());
    let after = store.local.bytes();
    assert_eq!(
        store.sets() - sets,
        usize::from(before != after),
        "one write for a change, none without"
    );
    assert_eq!(after, s.lock().running().map(|r| r.vm.ext().encode()));
}

#[test]
fn an_unchanged_record_is_never_copied_and_a_change_is_written_once() {
    let store = Counting::default();
    let s = Arc::new(Shared::new(Runtime::new(Box::new(store.clone()), Box::new(no_fetch))));
    let (mut m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
    assert_eq!(store.sets(), 1, "the start writes the record");
    for n in 1..=20 {
        m.put_u32(GAME_TIME + 8, TICK + n);
        step(&s, &store, |rt| {
            if let Some(work) = rt.pre_tick(&m) {
                rt.finish(work.tick, Some(true), Vec::new());
            }
        });
        if n % 3 == 0 {
            step(&s, &store, |rt| rt.kill(&m, 0, Some(1)));
        }
        step(&s, &store, |rt| rt.post_tick(&m, &[]));
    }
    assert_eq!(store.gets(), 0, "nothing else changed the record: no copy");
    // A revert puts an earlier record back: the next run copies it once
    // and resumes from it.
    let reverted = {
        let rt = s.lock();
        let r = rt.running().unwrap();
        let mut ext = r.vm.ext().clone();
        ext.tick = TICK;
        ext.encode()
    };
    store.local.put(reverted);
    step(&s, &store, |rt| rt.kill(&m, 0, Some(1)));
    assert_eq!(store.gets(), 1, "the changed record is copied once");
    assert_eq!(s.lock().running().unwrap().vm.ext().tick, TICK);
}

/// Guest memory that counts the reads of the players array's pointer
/// (`points::PLAYERS`): one per read of the roster.
struct CountingMemory<'a> {
    m: &'a dyn GuestMemory,
    players: std::cell::Cell<usize>,
}

impl<'a> CountingMemory<'a> {
    fn new(m: &'a dyn GuestMemory) -> Self {
        Self {
            m,
            players: std::cell::Cell::new(0),
        }
    }
}

impl GuestMemory for CountingMemory<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), engine::MemFault> {
        if addr == points::PLAYERS.base {
            self.players.set(self.players.get() + 1);
        }
        self.m.read(addr, buf)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), engine::MemFault> {
        Err(engine::MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
    }
}

#[test]
fn a_script_looping_the_players_reads_the_roster_once_per_moment() {
    let ir = ::rules::compile(
        r#"
rules "Roll" 1
type slayer
global var n: int = 0
on tick {
    for each players p {
        if p.alive { n = n + 10 }
        if p.team == team 1 { n = n + 1 }
        n = n + 100
    }
}
"#,
    )
    .unwrap();
    let (m, _) = guest([0; LEN], SLAYER);
    // What one roster read costs.
    let one = CountingMemory::new(&m);
    assert_eq!(read_players(&one).unwrap().len(), 2);
    let once = one.players.get();
    assert!(once > 0);
    let counted = CountingMemory::new(&m);
    let mut calls = Vec::new();
    let mut vm = Vm::new(ir, 0, SEED).unwrap();
    vm.begin_tick(TICK);
    {
        let mut g = GuestGame::new(&counted, &mut calls);
        vm.update(&mut g);
        assert_eq!(g.error, None);
    }
    // Two players with no unit (so not alive), on teams 0 and 1.
    assert_eq!(vm.global("n"), Some(201));
    assert_eq!(
        counted.players.get(),
        once,
        "the players, each one's alive and each one's team: one roster read"
    );
    assert!(calls.is_empty());
}
