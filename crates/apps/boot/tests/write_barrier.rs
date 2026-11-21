//! The D3D write barrier drains the watch before the entry runs, runs the
//! entry in place, and must not lose a store made while a wrapped entry is
//! in flight (between its entry drain and its return).

use std::sync::{Arc, Mutex};

use runtime::guest::Permissions;
use runtime::watch::CONTIGUOUS_BASE;
use runtime::{CpuAbi, DirtyRange, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi};

/// `D3DResource_AddRef`: one stdcall pointer, no live device needed.
const ADD_REF: u32 = 0x003f_b600;

fn page_of(rt: &Runtime, addr: u32) -> u32 {
    rt.memory().fold_address(addr).unwrap()
}

fn covers(dirty: &[DirtyRange], page: u32) -> bool {
    dirty.iter().any(|r| (r.addr..r.addr + r.len).contains(&page))
}

/// A real wrapped entry: the store made before the call is drained on
/// entry, the entry returns to its caller in one runner transfer (no
/// shadow call, no continuation), and its stdcall cleanup is the entry's.
#[test]
fn a_wrapped_entry_drains_first_and_runs_in_place() {
    let rt = Runtime::new(Guest::new(), Dispatch::default());
    {
        let mut m = rt.memory();
        m.map_zeroed(0x003f_0000, 0x14000, Permissions::RX).unwrap();
        m.map_zeroed(0x0100_0000, 0x4000, Permissions::RW).unwrap();
    }
    let _installed = boot::d3d_glue::install(&rt).unwrap();
    let id = rt.memory().write_watch_owners().into_iter().find(|w| w.1 == "boot:d3d").unwrap().0;

    let esp = 0x0100_1000;
    let mut cpu = CpuAbi { esp, ..Default::default() };
    {
        let mut m = rt.memory();
        m.set_u32(esp, runtime::runner::HARNESS_EXIT).unwrap();
        m.set_u32(esp + 4, 0x0100_0000).unwrap();
        m.set_u32(0x0100_0000, 1).unwrap();
        // A store before the call: the barrier drains it on entry.
        m.set_u32(0x0100_3000, 0xfeed_face).unwrap();
    }
    assert!(rt.memory().has_dirty(id));
    let mut ctx = ExecCtx::new(rt.clone(), 0);
    let t = Runner { max_transfers: 1 }.run(&mut cpu, &mut ctx, ADD_REF);
    assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit), "one transfer, straight back to the caller");
    assert_eq!(cpu.esp, esp + 8, "AddRef's stdcall cleanup");
    assert!(ctx.live_continuations().is_empty());
    assert_eq!(rt.memory().u32(esp).unwrap(), runtime::runner::HARNESS_EXIT, "the return slot keeps the caller's address");
    // Drained by the barrier, and nothing the call made since is lost:
    // whatever the entry stored itself is reported at the next entry.
    let left = rt.memory().take_dirty(id).unwrap();
    assert!(!covers(&left, page_of(&rt, 0x0100_3000)), "the store before the call was not drained: {left:x?}");
}

/// [`boot::d3d_glue::barrier_handler`] around an entry that calls guest
/// code before it returns: a store made during that call (here by the
/// callee) stays dirty after the entry returns, and the store before the
/// call went to `note` first.
#[test]
fn a_store_during_a_d3d_call_stays_dirty() {
    const ENTRY: u32 = 0x0001_0000;
    const CALLBACK: u32 = 0x0001_0100;
    let rt = Runtime::new(Guest::new(), Dispatch::default());
    let id = {
        let mut m = rt.memory();
        m.map_zeroed(0x0001_0000, 0x1000, Permissions::RX).unwrap();
        m.map_zeroed(0x0100_0000, 0x8000, Permissions::RW).unwrap();
        let id = m.subscribe_writes("test");
        m.watch_writes(id, CONTIGUOUS_BASE, 0x0400_0000).unwrap();
        id
    };
    let noted: Arc<Mutex<Vec<DirtyRange>>> = Arc::default();
    rt.edit_dispatch(|d| {
        // The callback the entry calls: a store, then `ret`.
        d.register_override(CALLBACK, "test: callback", |cpu, ctx, _| {
            let mut m = ctx.runtime().memory();
            m.set_u32(0x0100_6000, 0x1234_5678)?;
            m.ret(cpu, 0)
        });
        // An entry that returns through a guest call, as the D3D entries
        // that destroy resources or deliver push-buffer callbacks do.
        d.register_override(ENTRY, "test: entry", |cpu, ctx, _| {
            let rt = Arc::clone(ctx.runtime());
            let mut m = rt.memory();
            let ret_pc = m.u32(cpu.esp)?;
            cpu.esp = cpu.esp.wrapping_add(4);
            ctx.call_guest(cpu, &mut m, CALLBACK, move |_, _| Ok(TransferAbi::ret(ret_pc)))
        });
        let prev = d.override_handler(ENTRY).unwrap();
        let noted = Arc::clone(&noted);
        let note = move |dirty: &[DirtyRange]| noted.lock().unwrap().extend_from_slice(dirty);
        d.register_override(ENTRY, "test: barrier", boot::d3d_glue::barrier_handler(prev, id, note));
    });

    let esp = 0x0100_1000;
    let mut cpu = CpuAbi { esp, ..Default::default() };
    {
        let mut m = rt.memory();
        m.set_u32(esp, runtime::runner::HARNESS_EXIT).unwrap();
        m.set_u32(0x0100_3000, 0xfeed_face).unwrap();
    }
    let mut ctx = ExecCtx::new(rt.clone(), 0);
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit));
    assert_eq!(cpu.esp, esp + 4);
    assert!(ctx.live_continuations().is_empty());

    let noted = noted.lock().unwrap();
    assert!(covers(&noted, page_of(&rt, 0x0100_3000)), "the store before the call was not noted: {noted:x?}");
    assert!(!covers(&noted, page_of(&rt, 0x0100_6000)), "the callback's store came after the drain: {noted:x?}");
    let dirty = rt.memory().take_dirty(id).unwrap();
    assert!(covers(&dirty, page_of(&rt, 0x0100_6000)), "the in-flight store was discarded on return: {dirty:x?}");
}

// ---- flat RAM (contract slice F4) -------------------------------------------

extern "C" fn module_abi_version() -> u32 {
    runtime::cpu::ABI_VERSION
}

/// A dispatch with a flat-RAM module (module feature 16) that has no
/// functions: enough for the runtime to take the flat model, its window,
/// its cards and parking.
fn flat_dispatch() -> Dispatch {
    use runtime::generated::{FEATURE_ENTRY_TABLE, FEATURE_FLAT_RAM, H2GenFlatModule};
    use runtime::{H2GenEntry, H2GenEntryModule, H2GenModule, H2GenResume};
    let entries: &'static [H2GenEntry] = vec![H2GenEntry { addr: 0, kind: 0, handler: None, abi_version: None }].leak();
    let resumes: &'static [H2GenResume] = vec![H2GenResume { pc: 0, owner: 0, start: 0 }].leak();
    let module: &'static H2GenFlatModule = Box::leak(Box::new(H2GenFlatModule {
        entry: H2GenEntryModule {
            module: H2GenModule {
                abi_version: runtime::cpu::ABI_VERSION,
                entry_count: 0,
                resume_count: 0,
                features: FEATURE_ENTRY_TABLE | FEATURE_FLAT_RAM,
                entries: entries.as_ptr(),
                resumes: resumes.as_ptr(),
            },
            pcmap: std::ptr::null(),
            text_lo: 0,
            text_hi: 0,
        },
        fix_start: std::ptr::null(),
        fix_end: std::ptr::null(),
        fix_format: 0,
        fix_pad: 0,
    }));
    let mut d = Dispatch::default();
    // SAFETY: both tables hold their zero row, there is no text, no fixup
    // row, and everything is leaked.
    unsafe { d.register_generated_module(&module.entry.module, module_abi_version) }.unwrap();
    d
}

/// Contract slice F4: with a flat-RAM image, generated stores go through
/// the window and mark cards, not the paged model's dirty bitmap. The
/// barrier folds them at every draw: every page stored to between two
/// draws, through the window (here, the two card stores a generated store
/// makes, done by hand) or through Rust, reaches `note` at the second
/// draw, a crossing store with both its pages, and none of them again at
/// the draw after.
#[test]
fn barrier_sees_every_store_between_draws() {
    if !runtime::window::supported() {
        return;
    }
    const DRAW: u32 = 0x0001_0000;
    const DATA: u32 = 0x0100_0000;
    const STACK: u32 = DATA + 0xf000;
    let rt = Runtime::new(Guest::new(), flat_dispatch());
    assert!(rt.flat_ram());
    let id = {
        let mut m = rt.memory();
        m.map_zeroed(DRAW, 0x1000, Permissions::RX).unwrap();
        m.map_zeroed(DATA, 0x10000, Permissions::RW).unwrap();
        let id = m.subscribe_writes("test");
        m.watch_writes(id, CONTIGUOUS_BASE, 0x0400_0000).unwrap();
        id
    };
    let stats = rt.window_stats();
    assert_eq!((stats.aliased, stats.pages_outside), (1, 0), "RAM is in the window");
    let draws: Arc<Mutex<Vec<Vec<DirtyRange>>>> = Arc::default();
    rt.edit_dispatch(|d| {
        d.register_override(DRAW, "test: draw", |cpu, ctx, _| {
            let m = ctx.runtime().memory();
            m.ret(cpu, 0)
        });
        let prev = d.override_handler(DRAW).unwrap();
        let draws = Arc::clone(&draws);
        let note = move |dirty: &[DirtyRange]| draws.lock().unwrap().push(dirty.to_vec());
        d.register_override(DRAW, "test: barrier", boot::d3d_glue::barrier_handler(prev, id, note));
    });
    let base = rt.flat_base().unwrap();
    let cards = rt.flat_cards().unwrap();
    // A generated store: the store through the window, then its card and
    // its 4 MiB region's (RUNTIME-CONTRACT.md, "Flat RAM").
    let store = |va: u32, v: u32| {
        // SAFETY: the window holds every DATA page (checked above), and
        // the cards cover all 2^20 pages with 1,024 region bytes below.
        unsafe {
            (base.add(va as usize) as *mut u32).write_unaligned(v);
            cards.add((va >> 12) as usize).write_volatile(0);
            cards.sub(1024).add((va >> 22) as usize).write_volatile(0);
        }
    };
    let mut ctx = ExecCtx::new(rt.clone(), 0);
    let mut draw = || {
        rt.memory().set_u32(STACK, runtime::runner::HARNESS_EXIT).unwrap();
        let mut cpu = CpuAbi { esp: STACK, ..Default::default() };
        let t = Runner::default().run(&mut cpu, &mut ctx, DRAW);
        assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit));
    };
    draw();
    // Window stores, a crossing one among them; then a Rust store; then
    // nothing.
    let rounds: [(&[u32], &[u32]); 4] = [
        (&[DATA + 0x0010, DATA + 0x6000], &[]),
        (&[DATA + 0x2ffe], &[]),
        (&[], &[DATA + 0x9000]),
        (&[], &[]),
    ];
    let mut before: Vec<u32> = Vec::new();
    for (round, (window, rust)) in rounds.iter().enumerate() {
        for (i, &va) in window.iter().enumerate() {
            store(va, 0x5000 + (round * 16 + i) as u32);
        }
        for &va in rust.iter() {
            rt.memory().set_u32(va, 0x6000 + round as u32).unwrap();
        }
        for &va in window.iter() {
            assert_eq!(rt.memory().u32(va).unwrap() >> 12, 5, "the window store is in RAM");
        }
        let seen = draws.lock().unwrap().len();
        draw();
        let draws = draws.lock().unwrap();
        let noted: Vec<DirtyRange> = draws[seen..].concat();
        let mut now: Vec<u32> = window.iter().chain(rust.iter()).map(|&va| va & !0xfff).collect();
        if window.contains(&(DATA + 0x2ffe)) {
            now.push(DATA + 0x3000);
        }
        for &va in &now {
            assert!(covers(&noted, page_of(&rt, va)), "round {round}: {va:#x} not noted: {noted:x?}");
        }
        for &va in before.iter().filter(|va| !now.contains(va)) {
            // A card also reports the page after its own, so only pages
            // two apart from this round's are certain to stay out.
            if now.iter().any(|&n| n + 0x1000 == va) {
                continue;
            }
            assert!(!covers(&noted, page_of(&rt, va)), "round {round}: {va:#x} noted again: {noted:x?}");
        }
        before = now;
    }
}
