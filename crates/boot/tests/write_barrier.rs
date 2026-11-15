//! The D3D write barrier must not lose a store made while a wrapped entry
//! is in flight (between its entry drain and its return).

use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi};

/// `D3DResource_AddRef`: one stdcall pointer, no live device needed.
const ADD_REF: u32 = 0x003f_b600;

#[test]
fn a_store_during_a_d3d_call_stays_dirty() {
    let rt = Runtime::new(Guest::new(), Dispatch::default());
    {
        let mut m = rt.memory();
        m.map_zeroed(0x003f_0000, 0x14000, Permissions::RX).unwrap();
        m.map_zeroed(0x0100_0000, 0x4000, Permissions::RW).unwrap();
    }
    let installed = boot::d3d_glue::install(&rt).unwrap();
    let id = rt.memory().write_watch_owners().into_iter().find(|w| w.1 == "boot:d3d").unwrap().0;
    let shadow = installed.barrier_shadow(ADD_REF).expect("AddRef is wrapped");

    let mut cpu = CpuAbi { esp: 0x0100_1000, ..Default::default() };
    {
        let mut m = rt.memory();
        m.set_u32(cpu.esp, runtime::runner::HARNESS_EXIT).unwrap();
        m.set_u32(cpu.esp + 4, 0x0100_0000).unwrap();
        m.set_u32(0x0100_0000, 1).unwrap();
        let _ = m.take_dirty(id);
    }
    let mut ctx = ExecCtx::new(rt.clone(), 0);
    // Stop right after the wrapper's entry drain, before the real handler.
    let t = Runner { max_transfers: 1 }.run(&mut cpu, &mut ctx, ADD_REF);
    assert_eq!(ctx.event(t.event_id), Some(&Event::TransferLimit));
    let token = rt.memory().u32(cpu.esp).unwrap();

    // Another context's store while the call is in flight.
    rt.memory().set_u32(0x0100_3000, 0xfeed_face).unwrap();
    assert!(rt.memory().has_dirty(id));

    let t = Runner::default().run_from(&mut cpu, &mut ctx, TransferAbi::call(shadow, token));
    assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit));
    let mut m = rt.memory();
    let page = m.fold_address(0x0100_3000).unwrap();
    let dirty = m.take_dirty(id).unwrap();
    assert!(
        dirty.iter().any(|r| (r.addr..r.addr + r.len).contains(&page)),
        "the in-flight store was discarded on return: {dirty:x?}"
    );
}
