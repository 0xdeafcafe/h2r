//! The timeline hooks in the runtime and the kernel fire: a guest thread's
//! slice, an HLE call and an interrupt's delivery under `Machine`.
//! (Recording is process-global, so this file holds one test.)

use kernel::{Config, Kernel, Machine, RunOutcome};
use runtime::{
    CallConvention, CpuAbi, Dispatch, Guest, GuestInterrupt, Runtime, guest::Permissions,
};

const CODE: u32 = 0x0040_0000;
const IRQ: u32 = CODE + 0x100;
const STACK_BOTTOM: u32 = 0x6fff_0000;
const STACK_TOP: u32 = 0x7000_0000;

#[test]
fn slices_hle_calls_and_interrupts_reach_the_timeline() {
    let mut memory = Guest::new();
    memory.map_zeroed(CODE, 0x1_0000, Permissions::RWX).unwrap();
    memory
        .map_zeroed(STACK_BOTTOM, STACK_TOP - STACK_BOTTOM, Permissions::RW)
        .unwrap();
    let rt = Runtime::new(memory, Dispatch::default());
    let k = Kernel::install(&rt, Config::default()).unwrap();
    rt.edit_dispatch(|d| {
        d.register_override(CODE, "entry", |cpu, ctx, _| ctx.memory().ret(cpu, 0));
        d.register_override(IRQ, "irq", |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    });
    let cpu = CpuAbi {
        eip: CODE,
        esp: STACK_TOP - 4,
        ..CpuAbi::xbox_thread()
    };
    rt.memory()
        .set_u32(cpu.esp, runtime::exec::HARNESS_EXIT)
        .unwrap();
    let mut m = Machine::new(rt.clone(), k, cpu, STACK_BOTTOM, STACK_TOP).unwrap();
    rt.raise_interrupt(GuestInterrupt {
        target: IRQ,
        args: vec![],
        convention: CallConvention::Stdcall,
        thread: None,
        coalesce: false,
        name: "test irq",
    });
    timeline::start(timeline::Config::all());
    let outcome = m.run(100).unwrap();
    let t = timeline::stop();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert!(t.count("slice") >= 1, "no slice");
    assert!(t.count("hle call") >= 2, "entry and irq overrides");
    assert_eq!(t.count("test irq"), 2, "async begin and end");
    let mut json = Vec::new();
    t.write_chrome(&mut json).unwrap();
    let json = String::from_utf8(json).unwrap();
    assert!(json.contains("\"cat\":\"guest\""), "{json}");
    assert!(
        json.contains("\"ph\":\"b\"") && json.contains("\"ph\":\"e\""),
        "{json}"
    );
}
