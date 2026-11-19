//! The `native-post` feature (docs/graphics/native-renderer.md, stage 5):
//! its plan (one context-only hook the pipelined render runs on its own
//! thread, none with render-timing on, none with the switch off) and its
//! skip on synthetic guest memory: nothing is skipped without a host, for
//! a call the host does not accept, or when the device cannot take the
//! pass; the host sees what the call was asked to draw. A skipped call
//! leaves guest memory as the game's prologue does (a transcription of
//! `0x00013870`..`0x000138b3` and its callees, side by side with
//! `redo_prologue`), and the device gets the states the game's loop sets.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_post::{self, Host, NativePost};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::post::{PostFrame, SCENARIO, SCENARIO_SCREEN_EFFECTS, VIEW_KIND};

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    r.add(&NativePost).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_post"), "{text}");
}

#[test]
fn its_chain_runs_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_post=on").unwrap());
    assert!(r.add(&NativePost).unwrap());
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(text.contains("native_post_skip"), "{text}");
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.name == "RENDER_SCREEN_EFFECTS")
        .expect("placed at RENDER_SCREEN_EFFECTS");
    assert!(!chain.needs_shadow(), "nothing after the body");
    assert!(chain.render_thread().is_ok());
}

#[test]
fn render_timing_on_places_nothing() {
    let mut r = Registry::new(Switches::parse("impl.render.native_post=on,render-timing").unwrap());
    r.add(&NativePost).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_post"), "{text}");
}

/// Where the hook's ESP points (the return address; the arguments above).
const ESP: u32 = 0x0060_0000;
const SCENARIO_AT: u32 = 0x0058_0000;

#[derive(Default)]
struct Stub {
    accept: Mutex<bool>,
    open: Mutex<bool>,
    seen: Mutex<Vec<PostFrame>>,
    opened: Mutex<u32>,
    /// The prologue's states the device was given; `refuse_prologue`: the
    /// device sets none.
    states: Mutex<Vec<Vec<(u32, u32)>>>,
    refuse_prologue: Mutex<bool>,
}

impl Host for Stub {
    fn accepts(&self, _mem: &dyn GuestMemory, frame: &PostFrame) -> bool {
        self.seen.lock().unwrap().push(*frame);
        *self.accept.lock().unwrap()
    }
    fn prologue(&self, _mem: &mut dyn GuestMemory, states: &[(u32, u32)]) -> bool {
        if *self.refuse_prologue.lock().unwrap() {
            return false;
        }
        self.states.lock().unwrap().push(states.to_vec());
        true
    }
    fn open(&self, _frame: &PostFrame) -> bool {
        let open = *self.open.lock().unwrap();
        if open {
            *self.opened.lock().unwrap() += 1;
        }
        open
    }
}

/// The stack at the call: `0x00013870(late, colour)` pushes `colour`, then
/// `late`, then the return address.
fn call(m: &mut FlatMemory, late: bool, colour: bool) -> bool {
    m.set_u32(ESP, 0x0002_c33a).unwrap();
    // Only the low byte counts: the high bytes are whatever the push held.
    m.set_u32(ESP + 4, 0xdead_be00 | u32::from(late)).unwrap();
    m.set_u32(ESP + 8, 0xdead_be00 | u32::from(colour)).unwrap();
    native_post::on_screen_effects(m, ESP)
}

#[test]
fn a_call_is_skipped_only_when_the_host_draws_it() {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(VIEW_KIND, 1).unwrap();
    m.set_u32(SCENARIO, SCENARIO_AT).unwrap();
    m.set_u32(SCENARIO_AT + SCENARIO_SCREEN_EFFECTS, 1).unwrap();
    let before = native_post::counts();

    // No host: the call runs, and is counted.
    assert!(!call(&mut m, false, true));

    let stub = Arc::new(Stub::default());
    native_post::set_host(stub.clone());
    // Refused: the call runs; the host saw what it asked for.
    assert!(!call(&mut m, true, false));
    assert_eq!(
        stub.seen.lock().unwrap().last().copied(),
        Some(PostFrame {
            colour: false,
            late: true,
            view_kind: 1,
            screen_effects: 1,
            screen_effects_gated: true,
        })
    );
    // Accepted but the device takes no pass: the call runs.
    *stub.accept.lock().unwrap() = true;
    assert!(!call(&mut m, false, true));
    assert_eq!(*stub.opened.lock().unwrap(), 0);
    // Accepted and opened: skipped.
    *stub.open.lock().unwrap() = true;
    assert!(call(&mut m, true, true));
    assert_eq!(*stub.opened.lock().unwrap(), 1);
    // A view kind in 5..7 gates the screen effects off; still skipped.
    m.set_u32(VIEW_KIND, 6).unwrap();
    assert!(call(&mut m, false, true));
    assert!(!stub.seen.lock().unwrap().last().unwrap().screen_effects_gated);
    // An unreadable stack: the call runs, the host is not asked.
    let asked = stub.seen.lock().unwrap().len();
    assert!(!native_post::on_screen_effects(&mut m, 0x0001_0000));
    assert_eq!(stub.seen.lock().unwrap().len(), asked);

    let after = native_post::counts();
    assert_eq!(after.calls - before.calls, 6);
    assert_eq!(after.skipped - before.skipped, 2);
    assert_eq!(after.colour - before.colour, 4);
    assert_eq!(after.late - before.late, 2);
    assert_eq!(after.screen_effects - before.screen_effects, 4);
    assert_eq!(after.prologue_refused - before.prologue_refused, 0);
    // The skipped calls redid the prologue: the flag is set.
    assert_eq!(m.u8(native_render::post::PROLOGUE_FLAG).unwrap(), 1);
}

// The prologue, side by side.

const BASE: u32 = 0x0040_0000;
const SIZE: u32 = 0x0021_0000;
const S: u32 = 0x0048_5b48;
const LAST: u32 = 0x0059_0000;
const TABLE_PTR: u32 = 0x005a_0000;
const ENTRIES: u32 = 0x005b_0000;
const SCREEN: u32 = 0x0058_0000;

/// `0x00013870`..`0x000138b3` as the game runs it, instruction by
/// instruction (callees `0x00016b10`, `0x00018900`, `0x000245c0`,
/// `0x00024ee0` with argument 0), reading memory as it goes. Returns the
/// `(state, value)` pairs the inlined setter is called with.
fn game_prologue(m: &mut FlatMemory) -> Vec<(u32, u32)> {
    let mut states = Vec::new();
    m.set_u8(0x0046_70bc, 1).unwrap(); // 0x0001387b
    // 0x00016b10, ESI = 0x00485b48.
    let esi = S;
    let eax = m.u32(esi + 0x24).unwrap();
    if eax != 0 {
        // 0x00018900(esi, eax + 0x11c).
        let list = eax + 0x11c;
        let w = u32::from(m.u16(list).unwrap());
        let mut edi = (w & 0x1ff) * 5 + m.u32(m.u32(esi + 0x20).unwrap() + 0x14).unwrap();
        let mut ebp = 0;
        while ebp < u32::from(m.u16(list).unwrap()) >> 9 {
            states.push((u32::from(m.u8(edi).unwrap()), m.u32(edi + 1).unwrap()));
            edi += 5;
            ebp += 1;
        }
    }
    let f = m.u32(0x0045_dbd4).unwrap();
    for off in [0x0c, 0x10, 0x14, 0x18, 0x1c, 0x20, 0x24] {
        m.set_u32(esi + off, 0).unwrap();
    }
    m.set_u32(esi, u32::MAX).unwrap();
    m.set_u32(esi + 0x40, u32::MAX).unwrap();
    for off in [0x1320, 0x1360, 0x13a0, 0x13e0] {
        m.set_u32(esi + off, u32::MAX).unwrap();
        m.set_u32(esi + off + 0x10, f).unwrap();
    }
    // 0x00013887.
    let esi = m.u32(0x0048_5890).unwrap();
    if esi != 0 {
        let word = m.u16(0x0048_5600).unwrap() as i16 as i32;
        m.set_u32(esi + 0x1c, word as u32).unwrap();
        let esi = m.u32(0x0048_5890).unwrap();
        // 0x000245c0, EAX 0x0051ecf0, EBX 0: BX <= 0, so 0x00024642.
        m.set_u32(esi + 0x18, 0x0051_ecf0).unwrap();
        let (a, b) = (m.u32(0x0045_dc2c).unwrap(), m.u32(0x0045_dd30).unwrap());
        for (off, v) in [(0, 0), (4, a), (8, b), (0xc, 0), (0x10, a), (0x14, b)] {
            m.set_u32(esi + off, v).unwrap();
        }
        m.set_u32(esi + 0x20, 0).unwrap();
        // BX != 2 and [esi+0x20] & 7 == 0: 0x00024ee0(0), ECX = esi.
        assert_eq!(m.u32(esi + 0x20).unwrap() & 7, 0);
        let rec = if m.u32(0x004b_54c8).unwrap() != 0 && m.u8(0x004b_54d5).unwrap() == 0 {
            0x004b_5444
        } else {
            0
        };
        m.set_u32(rec + 0x10, 0).unwrap();
        m.set_u32(rec + 0xc, 0x0661_0629).unwrap();
        let idx = (m.u32(esi + 0x1c).unwrap() as i32).clamp(0, 1) as u32;
        let o = idx * 0x300;
        for i in 0..256u32 {
            // The curve goes to the stack; argument 0 != 1: the index.
            m.set_u8(0x004b_9570 + o + i, i as u8).unwrap();
            m.set_u8(0x004b_9470 + o + i, i as u8).unwrap();
            m.set_u8(0x004b_9370 + o + i, i as u8).unwrap();
        }
        // 0x0002468d.
        let c = m.u32(esi + 0x20).unwrap();
        m.set_u32(esi + 0x20, c + 1).unwrap();
    }
    states
}

/// Memory mid-frame: junk where the prologue stores, a last pass with a
/// three-entry list (entries unaligned), a screen record and the image's
/// floats.
fn prologue_memory(word: u16, screen: bool, gate: bool) -> FlatMemory {
    let mut m = FlatMemory::new(BASE, SIZE);
    m.fill(S, 0x1400, 0x77).unwrap();
    m.fill(0x004b_9370, 0x900, 0xaa).unwrap();
    m.fill(0x004b_5444, 0x20, 0x33).unwrap();
    m.set_u32(0x0045_dbd4, 1.0f32.to_bits()).unwrap();
    m.set_u32(0x0045_dc2c, 0.5f32.to_bits()).unwrap();
    m.set_u32(0x0045_dd30, 2.0f32.to_bits()).unwrap();
    m.set_u32(0x0046_70bc, 0x1122_3300).unwrap();
    m.set_u32(S + 0x24, LAST).unwrap();
    m.set_u32(S + 0x20, TABLE_PTR).unwrap();
    m.set_u32(TABLE_PTR + 0x14, ENTRIES).unwrap();
    // Count 3, first entry 2.
    m.set_u16(LAST + 0x11c, (3 << 9) | 2).unwrap();
    for (i, (state, value)) in [(57u8, 3u32), (96, 0), (144, 1)].into_iter().enumerate() {
        let at = ENTRIES + 10 + 5 * i as u32;
        m.set_u8(at, state).unwrap();
        m.set_u32(at + 1, value).unwrap();
    }
    m.set_u16(0x0048_5600, word).unwrap();
    if screen {
        m.set_u32(0x0048_5890, SCREEN).unwrap();
        m.fill(SCREEN, 0x40, 0x55).unwrap();
    }
    if gate {
        m.set_u32(0x004b_54c8, 0x0070_0000).unwrap();
    }
    m
}

fn all(m: &FlatMemory) -> Vec<u8> {
    m.bytes(BASE, SIZE as usize).unwrap()
}

#[test]
fn a_skipped_prologue_leaves_memory_as_the_games_does() {
    // Set 1 (word 5 clamps to 1), set 0 (word -1 clamps to 0), and no
    // screen record.
    for (word, screen) in [(5u16, true), (0xffff, true), (1, true), (0, false)] {
        let mut game = prologue_memory(word, screen, true);
        let mut port = prologue_memory(word, screen, true);
        let states = game_prologue(&mut game);
        let stub = Stub::default();
        assert!(native_post::redo_prologue(&stub, &mut port), "word {word:#x}");
        assert_eq!(*stub.states.lock().unwrap(), vec![states.clone()]);
        assert_eq!(states, vec![(57, 3), (96, 0), (144, 1)]);
        assert!(all(&game) == all(&port), "word {word:#x} screen {screen}: memory differs");
    }
    // No last pass: no states.
    let mut game = prologue_memory(1, true, true);
    let mut port = prologue_memory(1, true, true);
    for m in [&mut game, &mut port] {
        m.set_u32(S + 0x24, 0).unwrap();
    }
    assert!(game_prologue(&mut game).is_empty());
    let stub = Stub::default();
    assert!(native_post::redo_prologue(&stub, &mut port));
    assert_eq!(*stub.states.lock().unwrap(), vec![Vec::new()]);
    assert!(all(&game) == all(&port));
}

#[test]
fn a_prologue_that_cannot_be_redone_sets_nothing() {
    // The ramp record at address 0 (the gate off), outside this memory.
    let mut m = prologue_memory(1, true, false);
    let before = all(&m);
    let stub = Stub::default();
    assert!(!native_post::redo_prologue(&stub, &mut m));
    assert!(stub.states.lock().unwrap().is_empty(), "the device is not asked");
    assert!(all(&m) == before);
    // The device refuses its part.
    let mut m = prologue_memory(1, true, true);
    let before = all(&m);
    *stub.refuse_prologue.lock().unwrap() = true;
    assert!(!native_post::redo_prologue(&stub, &mut m));
    assert!(all(&m) == before);
}
