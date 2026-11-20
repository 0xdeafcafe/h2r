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
