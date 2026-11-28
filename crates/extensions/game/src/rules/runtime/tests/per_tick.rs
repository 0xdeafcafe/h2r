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
    f(&mut s.lock());
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
    let s = Arc::new(Shared::new(Runtime::new(
        Box::new(store.clone()),
        Box::new(no_fetch),
    )));
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
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), guest_layout::MemFault> {
        if addr == points::PLAYERS.base {
            self.players.set(self.players.get() + 1);
        }
        self.m.read(addr, buf)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), guest_layout::MemFault> {
        Err(guest_layout::MemFault {
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

/// Scores a kill, counts kills into a HUD widget (so the show state
/// changes at each) and counts ticks.
const TOUR: &str = r#"
rules "Tour" 1
type slayer
requires hud.widgets
hud held: text at koth.time
global var kills: int = 0
global var ticks: int = 0
on kill(victim, killer) {
    if killer != none { score(killer, 2) }
    kills = kills + 1
    hud_value(held, kills)
}
on tick {
    ticks = ticks + 1
}
"#;

#[test]
fn three_hundred_ticks_keep_the_record_the_calls_and_the_show_state() {
    let store = Counting::default();
    let ir = ::rules::compile(TOUR).unwrap();
    let blob = ir.encode();
    let s = Arc::new(Shared::new(Runtime::new(
        Box::new(store.clone()),
        Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())),
    )));
    let (mut m, _) = guest(block(prefix(&ir.id()), 0), SLAYER);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    let mut works = Vec::new();
    for n in 1..=300u32 {
        m.put_u32(GAME_TIME + 8, TICK + n);
        step(&s, &store, |rt| {
            if let Some(work) = rt.pre_tick(&m) {
                rt.finish(work.tick, Some(true), Vec::new());
                works.push(work);
            }
        });
        if n % 7 == 0 {
            let killer = ((n / 7) % 2) as u8;
            step(&s, &store, |rt| rt.kill(&m, 1 - killer, Some(killer)));
        }
        step(&s, &store, |rt| rt.post_tick(&m, &[]));
        // The show state offered this tick is the VM's, encoded.
        let rt = s.lock();
        let r = rt.running().unwrap();
        let offered = rt.show_cue.last.as_ref().map(|(_, bytes)| bytes.clone());
        assert_eq!(offered, Some(r.vm.show().encode()), "tick {n}");
    }
    // The first boundary asks for the authority; each kill's score is
    // made at the boundary after it, in order.
    let mut expected = vec![Work {
        tick: TICK + 2,
        ask_authority: true,
        calls: Vec::new(),
        sight: Vec::new(),
    }];
    for k in 1..=42u32 {
        expected.push(Work {
            tick: TICK + 7 * k + 2,
            ask_authority: false,
            calls: vec![Call::Score {
                player: (k % 2) as u8,
                delta: 2,
            }],
            sight: Vec::new(),
        });
    }
    assert_eq!(works, expected);
    let rt = s.lock();
    let r = rt.running().unwrap();
    assert_eq!(r.vm.global("kills"), Some(42));
    assert_eq!(r.vm.global("ticks"), Some(300));
    assert_eq!(r.vm.ext().tick, TICK + 301);
    assert_eq!(store.local.bytes(), Some(r.vm.ext().encode()));
    assert_eq!(store.gets(), 0, "nothing else changed the record");
    // The show state was encoded at the first tick and again at each
    // kill, never at a tick that left it as it was.
    assert_eq!(rt.show_cue.encodes, 43);
}

#[test]
fn the_show_state_is_encoded_again_after_a_change_and_after_it_was_forgotten() {
    let mut vm = Vm::new(::rules::compile(TOUR).unwrap(), 0, 0).unwrap();
    let mut g = ::rules::fake::FakeGame::with_players(2);
    vm.begin_tick(0);
    vm.fire(
        &mut g,
        Input::Kill {
            victim: 0,
            killer: Some(1),
        },
    );
    let (quiet, shown) = (::rules::show::State::default(), vm.show().clone());
    assert_ne!(quiet, shown);
    let mut cue = ShowCue::default();
    assert_eq!(cue.bytes(&quiet), quiet.encode());
    assert_eq!(cue.bytes(&quiet), quiet.encode());
    assert_eq!(cue.encodes, 1, "the same state is not encoded again");
    assert_eq!(cue.bytes(&shown), shown.encode());
    assert_eq!(cue.encodes, 2, "a changed state is");
    cue.forget();
    assert_eq!(cue.bytes(&shown), shown.encode());
    assert_eq!(cue.encodes, 3, "and so is any state after a forget");
}

/// Offers one picker at the round's start, and keeps it.
const OFFER: &str = r#"
rules "Offer" 1
type slayer
requires player.pick
on round_start {
    offer_pick(0, "LOADOUT")
    pick_row(0, "A")
    pick_row(0, "B")
}
"#;

#[test]
fn a_standing_picker_is_sent_at_once_and_again_every_pick_repeat_ticks() {
    let store = Local::default();
    let (s, mut m) = playing(&store, OFFER);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    let mut last = None;
    for n in 1..=200u32 {
        m.put_u32(GAME_TIME + 8, TICK + n);
        let work = s.lock().pre_tick(&m);
        if let Some(work) = work {
            s.lock().finish(work.tick, Some(true), Vec::new());
        }
        s.lock().post_tick(&m, &[]);
        let tick = TICK + n;
        if last.is_none_or(|at: u32| tick - at >= ::rules::show::PICK_REPEAT) {
            last = Some(tick);
        }
        let sent = s
            .lock()
            .pick_cue
            .as_ref()
            .map(|(offers, at)| (offers.len(), *at));
        assert_eq!(sent, last.map(|at| (1, at)), "tick {n}");
    }
    assert_eq!(last, Some(TICK + 1 + 2 * ::rules::show::PICK_REPEAT));
}
