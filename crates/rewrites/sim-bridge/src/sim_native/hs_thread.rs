//! Check an ordinary script from its root frame to completion. A prediction
//! uses the real VM, seeded with the global cells that expression can reach.
//! Only synchronous scalar scripts can run here. The guest executes once.
use super::hs_parity::{self, THREAD};
use engine::{datum::layout, hs_pool, hs_syntax, points};
use hooks::Builder;
use hs::vm::{Cell, HsType, Native, ThreadState, Vm, VmImage};
use hs::{Catalog, Datum, Global, Node, Scenario, Script};
use runtime::{CpuAbi, Guest};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const ENABLED: u32 = 0x004f_9388;
const THREAD_BYTES: u32 = 0x418;
// Count visits, including repeated edges in a DAG and recursive script calls.
const MAX_VISITS: usize = 64;

struct Program {
    scenario: Scenario,
    catalog: Catalog,
    image: VmImage,
}

impl Program {
    fn load(g: &Guest, base: u32) -> Option<Self> {
        let read = |at, len| {
            let mut buf = vec![0; len];
            g.read_bytes(at, &mut buf, 1)
                .map_err(|_| hs::Error::Invalid("unmapped HS table".into()))?;
            Ok(buf)
        };
        Some(Self {
            scenario: Scenario::read_memory(base, &read).ok()?,
            catalog: Catalog::from_memory(&read, &|at| (0x1000..0x400000).contains(&at)).ok()?,
            image: VmImage::without_stubs(),
        })
    }
}

pub(super) fn install(b: &mut Builder) {
    // Remember decode failures too. A hot script should not retry a full map
    // decode every tick. A new map can reuse every address of its predecessor.
    let cache = Arc::new(Mutex::new(None::<(u32, Option<Program>)>));
    let invalidate = Arc::clone(&cache);
    b.raw()
        .observe_before(
            &points::HS_NEW_MAP,
            "discard the HS thread check's map",
            move |_| {
                *invalidate.lock().unwrap() = None;
            },
        )
        .optional();
    hs_parity::observe(
        b,
        &points::HS_THREAD_EXECUTE,
        &THREAD,
        0,
        move |cpu, g| {
            let start = Start::read(cpu, g)?;
            let mut cached = cache.lock().unwrap();
            if cached
                .as_ref()
                .is_none_or(|(base, _)| *base != start.scenario)
            {
                *cached = Some((start.scenario, Program::load(g, start.scenario)));
            }
            plan(g, &start, cached.as_ref()?.1.as_ref()?)
        },
        compare,
    );
}

fn pool_record(g: &Guest, point: u32, slot: u32, stride: u32) -> Option<u32> {
    let header = g.u32(point).ok()?;
    if header == 0
        || g.u32(header.checked_add(layout::ELEMENT_SIZE)?).ok()? != stride
        || slot >= g.u32(header.checked_add(layout::MAXIMUM_COUNT)?).ok()?
    {
        return None;
    }
    let data = g.u32(header.checked_add(layout::DATA)?).ok()?;
    (data != 0).then_some(())?;
    data.checked_add(slot.checked_mul(stride)?)
}

fn definition(g: &Guest, sc: u32, offset: u32, index: u16, stride: usize) -> Option<u32> {
    let header = sc.checked_add(offset)?;
    if u32::from(index) >= g.u32(header).ok()? {
        return None;
    }
    let defs = g.u32(header.checked_add(4)?).ok()?;
    (defs != 0).then_some(())?;
    defs.checked_add(u32::from(index) * stride as u32)
}

struct Start {
    thread: u32,
    script: u16,
    tick: i32,
    scenario: u32,
}

impl Start {
    fn read(cpu: &CpuAbi, g: &Guest) -> Option<Self> {
        // The VM's single-precision operations assume nearest rounding, masked
        // exceptions, and neither flush-to-zero nor denormals-are-zero.
        if cpu.mxcsr & 0xffc0 != 0x1f80 || g.u8(ENABLED).ok()? == 0 {
            return None;
        }
        let handle = cpu.edi;
        let thread = pool_record(g, hs_pool::THREADS, handle & 0xffff, THREAD_BYTES)?;
        thread.checked_add(THREAD_BYTES)?;
        if handle >> 16 == 0 || u32::from(g.u16(thread).ok()?) != handle >> 16
            || g.u16(thread + 2).ok()? != 0 // ordinary category, no pending flags
            || g.u32(thread + 0x10).ok()? != thread + 0x18
        {
            return None;
        }
        let script = u16::try_from(g.u32(thread + 4).ok()?).ok()?;
        let time = g.u32(points::GAME_TIME.base).ok()?;
        let scenario = g.u32(points::SCENARIO.base).ok()?;
        if scenario == 0 || time == 0 {
            return None;
        }
        let tick = g.u32(time.checked_add(8)?).ok()? as i32;
        (tick >= 0).then_some(Self {
            thread,
            script,
            tick,
            scenario,
        })
    }
}

struct Expected {
    thread: u32,
    script: u16,
    result: Cell,
    wake: i32,
    globals: Vec<(u32, Cell)>,
}

fn current_script<'a>(g: &Guest, base: u32, program: &'a Program, i: u16) -> Option<&'a Script> {
    let def = program.scenario.scripts().get(usize::from(i))?;
    let at = definition(g, base, 0x1b8, i, Script::SIZE)?;
    (g.read::<{ Script::SIZE }>(at).ok()? == def.encode()).then_some(def)
}

fn scalar(ty: u16) -> bool {
    matches!(
        HsType(ty),
        HsType::BOOLEAN | HsType::REAL | HsType::SHORT | HsType::LONG
    )
}

fn current_builtin(g: &Guest, program: &Program, index: u16) -> Option<Native> {
    let desc = program.catalog.function(index).ok()?;
    let at = g.u32(hs::FUNCTION_TABLE + u32::from(index) * 4).ok()?;
    if at == 0
        || at != desc.descriptor()
        || g.u16(at).ok()? != desc.return_type()
        || g.u16(at.checked_add(2)?).ok()? != desc.flags()
        || g.u32(at.checked_add(4)?).ok()? != desc.evaluate()
        || usize::from(g.u16(at.checked_add(12)?).ok()?) != desc.parameters().len()
        || !(3..=8).contains(&desc.return_type())
    {
        return None;
    }
    for (i, &ty) in desc.parameters().iter().enumerate() {
        if g.u16(at.checked_add(14 + i as u32 * 2)?).ok()? != ty {
            return None;
        }
    }
    match Native::at(desc.evaluate())? {
        n @ (Native::Begin
        | Native::If
        | Native::Set
        | Native::AndOr
        | Native::Arithmetic
        | Native::Equal
        | Native::Compare
        | Native::Not
        | Native::Pin) => Some(n),
        _ => None,
    }
}

fn plan(g: &Guest, start: &Start, program: &Program) -> Option<Expected> {
    let script = current_script(g, start.scenario, program, start.script)?;
    if script.kind() > 2 {
        return None;
    }
    let mut globals = BTreeMap::new();
    let mut pending = vec![script.root()];
    // Recheck every input a cached expression can reach against the live
    // program. This also covers changes from hot edits and snapshot loads.
    for _ in 0..MAX_VISITS {
        let Some(d) = pending.pop() else { break };
        let node = *program.scenario.node(d).ok()?;
        let at = pool_record(g, hs_syntax::NODES, d.slot() as u32, Node::SIZE as u32)?;
        if g.read::<{ Node::SIZE }>(at).ok()? != node.encode() {
            return None;
        }
        if node.next() != Datum::NULL {
            pending.push(node.next());
        }
        if node.value_type() == HsType::FUNCTION_NAME.0 && node.atom() && !node.global() {
            continue;
        }
        if !(3..=8).contains(&node.value_type()) {
            return None;
        }
        if !node.atom() {
            pending.push(Datum(node.data()));
            if node.script_call() {
                let callee = current_script(g, start.scenario, program, node.opcode())?;
                if !(3..=8).contains(&callee.return_type()) {
                    return None;
                }
                pending.push(callee.root());
            } else if current_builtin(g, program, node.opcode())? == Native::Set {
                let head = program.scenario.node(Datum(node.data())).ok()?;
                // The set evaluator takes the target's raw index, without
                // evaluating its node. Require that the dependency walk will
                // account for the exact local cell the evaluator writes.
                if !program.scenario.node(head.next()).ok()?.global() {
                    return None;
                }
            }
        } else if node.global() {
            let i = u16::try_from(node.data()).ok()?;
            if i & 0x8000 != 0 {
                return None;
            }
            let at = definition(g, start.scenario, 0x1c0, i, Global::SIZE)?;
            let ty = g.u16(at.checked_add(0x20)?).ok()?;
            if !scalar(ty) || ty != program.scenario.globals().get(usize::from(i))?.value_type() {
                return None;
            }
            let cell = pool_record(g, hs_pool::GLOBALS, hs_pool::EXTERNALS + u32::from(i), 8)?
                .checked_add(4)?;
            globals.insert(i, (cell, Cell(g.u32(cell).ok()?)));
        } else if !scalar(node.opcode()) && node.opcode() != HsType::VOID.0 {
            return None;
        }
    }
    if !pending.is_empty() {
        return None;
    }

    let mut vm = Vm::new(&program.scenario, &program.catalog, &program.image);
    vm.set_enabled(true);
    for (&i, &(_, value)) in &globals {
        vm.set_global(usize::from(i), value);
    }
    let slot = vm.create_script_thread(start.script).ok()?;
    vm.wake_script(start.script).ok()?;
    // The whitelist above excludes engine calls, external cells, object
    // conversions and RNG draws. Only the engine clock participates.
    let mut engine = hs::vm::mock::RecordingEngine::new(hs::vm::mock::Policy::Neutral);
    engine.time = start.tick;
    vm.update(&mut engine);
    if !vm.faults().is_empty()
        || !engine.counts.is_empty()
        || engine.object_queries != 0
        || !engine.external_reads.is_empty()
        || !engine.external_writes.is_empty()
    {
        return None;
    }
    let thread = vm.thread(slot)?;
    let result = thread.root_result()?;
    let wake = match thread.state() {
        ThreadState::Finished => hs::vm::FINISHED,
        ThreadState::Waiting(wake) => wake,
        _ => return None,
    };
    Some(Expected {
        script: start.script,
        thread: start.thread,
        result,
        wake,
        globals: globals
            .into_iter()
            .map(|(i, (cell, _))| Some((cell, vm.global(usize::from(i))?)))
            .collect::<Option<_>>()?,
    })
}

fn compare(expected: &Expected, _: &CpuAbi, g: &Guest) -> Option<String> {
    let root = expected.thread + 0x18;
    let word = |what: &str, at: u32, want: u32| {
        let have = g.u32(at).ok();
        (have != Some(want)).then(|| {
            format!(
                "script {} {what} at {at:#x}: expected {want:#x}, read {have:x?}",
                expected.script
            )
        })
    };
    word("wake", expected.thread + 8, expected.wake as u32)
        .or_else(|| word("frame", expected.thread + 0x10, root))
        .or_else(|| word("result", root + 0xe, expected.result.0))
        .or_else(|| {
            let cursor = g.u16(root + 0xc).ok();
            (cursor != Some(4)).then(|| {
                format!(
                    "script {} root cursor: expected 4, read {cursor:?}",
                    expected.script
                )
            })
        })
        .or_else(|| {
            expected
                .globals
                .iter()
                .find_map(|&(at, value)| word("global", at, value.0))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hs::fixture::{Builtin, Exe, Tables};
    use runtime::guest::Permissions;

    const BASE: u32 = 0x0060_0000;
    const THREAD: u32 = BASE + 0x1800;
    const CELLS: u32 = BASE + 0x2000;
    const CELL: u32 = CELLS + hs_pool::EXTERNALS * 8 + 4;
    const VALUE: u32 = 0xe765_4321;

    fn fixture(kind: u16) -> (Guest, CpuAbi, u32) {
        let mut g = Guest::new();
        let exe = Exe {
            builtins: vec![Builtin {
                returns: HsType::PASSTHROUGH.0,
                flags: 2,
                ..Builtin::default()
            }],
            ..Exe::default()
        };
        // Map the single synthetic section emitted by Exe::xbe.
        let bytes = exe.xbe();
        g.map_zeroed(0x0020_0000, 0x0028_0000, Permissions::RW)
            .unwrap();
        g.write_bytes(0x0020_0000, &bytes[0x400..], 1).unwrap();
        let desc = g.u32(hs::FUNCTION_TABLE).unwrap();
        let addr = Native::BY_ADDRESS
            .iter()
            .find(|(_, op)| *op == Native::Set)
            .unwrap()
            .0;
        g.set_u32(desc + 4, addr).unwrap();
        let mut t = Tables::new();
        let ty = HsType::LONG.0;
        for name in ["dest", "source"] {
            let zero = t.value(ty, 0);
            t.global_definition(name, ty, zero);
        }
        let dest = t.global(0, "dest", ty);
        let src = t.global(1, "source", ty);
        let root = t.call(0, "assign", ty, &[dest, src]);
        t.script("copy", kind, ty, root);
        let mut payload = vec![0; 0x1000];
        t.write(&mut payload, BASE, 0x240);

        for point in [
            points::SCENARIO.base,
            hs_pool::THREADS,
            points::GAME_TIME.base,
        ] {
            g.map_zeroed(point & !0xfff, 0x1000, Permissions::RW)
                .unwrap();
        }
        g.map_zeroed(BASE, 0x5000, Permissions::RW).unwrap();
        g.write_bytes(BASE, &payload, 1).unwrap();
        let nodes = g.u32(BASE + 0x23c).unwrap();
        let count = g.u32(BASE + 0x238).unwrap();
        for (point, header, data, stride, count) in [
            (hs_pool::THREADS, BASE + 0x1000, THREAD, THREAD_BYTES, 1),
            (
                hs_syntax::NODES,
                BASE + 0x1100,
                nodes,
                Node::SIZE as u32,
                count,
            ),
            (
                hs_pool::GLOBALS,
                BASE + 0x1200,
                CELLS,
                8,
                hs_pool::EXTERNALS + 2,
            ),
        ] {
            g.set_u32(point, header).unwrap();
            g.set_u32(header + layout::MAXIMUM_COUNT, count).unwrap();
            g.set_u32(header + layout::DATA, data).unwrap();
            g.set_u32(header + layout::ELEMENT_SIZE, stride).unwrap();
        }
        g.set_u8(ENABLED, 1).unwrap();
        g.set_u32(points::SCENARIO.base, BASE).unwrap();
        g.set_u32(points::GAME_TIME.base, BASE + 0x1400).unwrap();
        g.set_u16(THREAD, 0x8123).unwrap();
        g.set_u32(THREAD + 0x10, THREAD + 0x18).unwrap();
        g.set_u32(CELL + 8, VALUE).unwrap();
        let cpu = CpuAbi {
            edi: 0x8123_0000,
            mxcsr: 0x1f80,
            ..CpuAbi::default()
        };
        (g, cpu, nodes + root.slot() as u32 * Node::SIZE as u32)
    }

    #[test]
    fn predicts_a_script_root_and_its_stores_without_applying_them() {
        for (kind, wake) in [(0, u32::MAX), (1, u32::MAX), (2, 0)] {
            let (mut g, cpu, _) = fixture(kind);
            let start = Start::read(&cpu, &g).unwrap();
            let program = Program::load(&g, BASE).unwrap();
            let expected = plan(&g, &start, &program).unwrap();
            assert_eq!(g.u32(CELL).unwrap(), 0);

            g.set_u32(THREAD + 8, wake).unwrap();
            g.set_u16(THREAD + 0x18 + 0xc, 4).unwrap();
            g.set_u32(THREAD + 0x18 + 0xe, VALUE).unwrap();
            assert!(compare(&expected, &cpu, &g).unwrap().contains("global"));
            g.set_u32(CELL, VALUE).unwrap();
            assert_eq!(compare(&expected, &cpu, &g), None);
            g.set_u32(THREAD + 8, 9).unwrap();
            assert!(compare(&expected, &cpu, &g).unwrap().contains("wake"));
        }
    }

    #[test]
    fn cached_program_declines_edits_unsupported_evaluators_and_suspended_frames() {
        let (mut g, cpu, root) = fixture(0);
        let start = Start::read(&cpu, &g).unwrap();
        let program = Program::load(&g, BASE).unwrap();
        let desc = g.u32(hs::FUNCTION_TABLE).unwrap();
        let sleep = Native::BY_ADDRESS
            .iter()
            .find(|(_, op)| *op == Native::Sleep)
            .unwrap()
            .0;
        g.set_u32(desc + 4, sleep).unwrap();
        assert!(plan(&g, &start, &program).is_none());
        let sleeping = Program::load(&g, BASE).unwrap();
        assert!(plan(&g, &start, &sleeping).is_none());
        g.set_u32(desc + 4, program.catalog.function(0).unwrap().evaluate())
            .unwrap();
        g.set_u16(root + 4, HsType::SHORT.0).unwrap();
        assert!(plan(&g, &start, &program).is_none());
        g.set_u32(THREAD + 0x10, THREAD + 0x40).unwrap();
        assert!(Start::read(&cpu, &g).is_none());
    }
}
