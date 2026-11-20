//! A unit callback can inspect its actual return PC. At each RET dispatch
//! must enter the port at that site without running the entry chain again.
use engine::{points, sim_physics::PORTS};
use feature::{Base, Registry, Switches, sim_physics::NativePhysics};
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, exec::HARNESS_EXIT, guest::Permissions};
use std::sync::{Arc, Mutex};

#[test]
fn callback_returns_use_real_guest_stack() {
    let port = PORTS.iter().find(|p| p.point.base == points::UNIT_UPDATE.base).unwrap();
    let mut mem = Guest::new();
    mem.map_zeroed(0xb0000, 0x80000, Permissions::RX).unwrap();
    let stack = 0x703000;
    mem.map_zeroed(stack - 0x1000, 0x2000, Permissions::RW).unwrap();
    mem.set_u32(stack, HARNESS_EXIT).unwrap();
    mem.set_u32(stack + 4, 0x9000_0032).unwrap();
    let mut dispatch = Dispatch::default();
    dispatch.register_generated_rust(port.point.base, |_, _, _| {
        panic!("unit original reached with port enabled")
    }).unwrap();
    for &site in port.resumes {
        dispatch.register_resume(site, port.point.base, site).unwrap();
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    for (target, cleanup) in [
        (0xc49b0, 4), (0xc58f0, 4), (0xc60c0, 4), (0xc5eb0, 0), (0xc6740, 0),
        (0x114c60, 0), (0xc6990, 0), (0xc6810, 0), (0xc50a0, 0),
    ] {
        let calls = Arc::clone(&calls);
        dispatch.register_override(target, "unit fixture", move |cpu, ctx, _| {
            let mut calls = calls.lock().unwrap();
            let mem = ctx.memory();
            if cleanup != 0 {
                assert_eq!(mem.u32(cpu.esp + 4)?, 0x9000_0032);
            } else {
                assert_eq!(cpu.eax, 0x9000_0032);
            }
            calls.push(mem.u32(cpu.esp)?);
            cpu.eax = 0x4400_0000 | (1 << ((calls.len() - 1) % 8));
            mem.ret(cpu, cleanup)
        });
    }
    let rt = Runtime::new(mem, dispatch);
    let mut registry = Registry::new(Switches::defaults());
    registry.add(&NativePhysics).unwrap();
    assert!(registry.build(&Base).unwrap().chains.is_empty());
    let mut registry = Registry::new(Switches::parse("impl.sim.native_unit_update").unwrap());
    registry.add(&NativePhysics).unwrap();
    registry.build(&Base).unwrap().install(&rt).unwrap();
    let mut cpu = CpuAbi { esp: stack, ebx: 17, esi: 25, ..Default::default() };
    let mut ctx = ExecCtx::new(rt, 1);
    let done = Runner::default().run(&mut cpu, &mut ctx, port.point.base);
    assert!(matches!(ctx.event(done.event_id), Some(Event::HarnessExit)));
    assert_eq!(&*calls.lock().unwrap(), port.resumes);
    assert_eq!((cpu.eax, cpu.ebx, cpu.esi, cpu.esp), (0x4400_001f, 17, 25, stack + 8));
}

#[test]
fn disabled_port_returns_to_generated_owner_with_its_selector() {
    let port = PORTS.iter().find(|p| p.point.base == points::UNIT_UPDATE.base).unwrap();
    let first = port.resumes[0];
    let stack = 0x703000;
    let mut mem = Guest::new();
    mem.map_zeroed(0xb0000, 0x80000, Permissions::RX).unwrap();
    mem.map_zeroed(stack - 0x1000, 0x2000, Permissions::RW).unwrap();
    mem.set_u32(stack, HARNESS_EXIT).unwrap();
    mem.set_u32(stack + 4, 17).unwrap();
    let mut dispatch = Dispatch::default();
    // A module may assign a shared return site to another canonical owner.
    let owner = 0xc6800;
    let selector = owner+12;
    dispatch.register_generated_rust(port.point.base, |_,_,_| panic!("unexpected entry")).unwrap();
    dispatch.register_generated_rust(owner, move |cpu,ctx,pc| {
        assert_eq!(pc, selector);
        let memory = ctx.memory();
        cpu.esi = memory.u32(cpu.esp)?;
        cpu.ebx = memory.u32(cpu.esp+4)?;
        cpu.esp += 8;
        cpu.eax = 91;
        memory.ret(cpu,4)
    }).unwrap();
    for &pc in port.resumes {
        dispatch.register_resume(pc,owner,if pc==first {selector} else {pc}).unwrap();
    }
    dispatch.register_override(0xc49b0, "disable unit port", move |cpu,ctx,_| {
        assert!(ctx.runtime().edit_dispatch(|d| d.set_enabled(port.point.base,false)));
        ctx.memory().ret(cpu,4)
    });
    let rt = Runtime::new(mem,dispatch);
    let mut registry = Registry::new(Switches::parse("impl.sim.native_unit_update").unwrap());
    registry.add(&NativePhysics).unwrap();
    registry.build(&Base).unwrap().install(&rt).unwrap();
    let mut ctx = ExecCtx::new(rt,1);
    let mut cpu = CpuAbi {esp:stack, esi:19, ebx:23, ..Default::default()};
    let done = Runner::default().run(&mut cpu,&mut ctx,port.point.base);
    assert!(matches!(ctx.event(done.event_id),Some(Event::HarnessExit)));
    assert_eq!((cpu.eax,cpu.esi,cpu.ebx,cpu.esp),(91,19,23,stack+8));
}

#[test]
fn missing_continuation_leaves_the_plan_uninstalled() {
    let port = PORTS.iter().find(|p| p.point.base == points::UNIT_UPDATE.base).unwrap();
    let mut dispatch = Dispatch::default();
    dispatch.register_generated_rust(port.point.base, |_,_,_| panic!("installation only")).unwrap();
    for &site in &port.resumes[1..] {
        dispatch.register_resume(site,port.point.base,site).unwrap();
    }
    let rt = Runtime::new(Guest::new(),dispatch);
    let mut registry = Registry::new(Switches::parse("impl.sim.native_unit_update").unwrap());
    registry.add(&NativePhysics).unwrap();
    let result = registry.build(&Base).unwrap().install(&rt);
    assert!(matches!(result,Err(feature::InstallError::MissingResume { site, .. }) if site==port.resumes[0]));
    assert!(rt.edit_dispatch(|d| d.override_name(port.point.base).is_none()));
}
