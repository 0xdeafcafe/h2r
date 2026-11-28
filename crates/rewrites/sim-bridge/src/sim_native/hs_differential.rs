//! Run an invented scenario on the Rust VM and through the owner's lifted
//! map initializer and scheduler. The test supplies the same clock and AI
//! query value to both engines. All guest evaluators and their callees run.
use guest_layout::datum::{DatumArray, layout};
use guest_layout::{GuestMemory, points};
use engine::{hs_pool, hs_syntax};
use hs::Catalog;
use hs::fixture::Tables;
use hs::vm::mock::{Policy, RecordingEngine};
use hs::vm::{Cell, HsType, Native, ThreadState, Vm, VmImage};
use ir::interp::{Cpu, FlatMemory};
use lifted::Original;
use lifted::image::Image;
use runtime::exec::HARNESS_EXIT;
use std::collections::BTreeMap;

// Owned fixture addresses in the test interpreter's flat RAM.
const ARENA: u32 = 0x0090_0000;
const SCENARIO: u32 = ARENA + 0x1000;
const THREADS: u32 = ARENA + 0x3000;
const GLOBALS: u32 = ARENA + 0x6000;
const NODES: u32 = ARENA + 0x9000;
const LISTS: u32 = ARENA + 0xa000;
const TIME: u32 = ARENA + 0xb000;
const GAME: u32 = ARENA + 0xc000;
const AI: u32 = ARENA + 0xe000;
const STACK: u32 = ARENA + 0x1fff0;

// Guest research points: map-loaded guard, object-list cleanup at tick end,
// AI query and its source byte, cast-table initializer, indirect dispatch.
const SCENARIO_INDEX: u32 = 0x0046_86c0;
const OBJECT_LISTS: u32 = 0x004f_55d8;
const AI_QUERY: u32 = 0x002a_4d40;
const AI_GLOBALS: u32 = 0x004f_55d0;
const CASTS_INITIALIZE: u32 = 0x0020_a1a0;
const DISPATCH_BUILTIN: u32 = 0x0020_995b;
const DISPATCH_SCRIPT: u32 = 0x0020_996b;

fn tables(catalog: &Catalog) -> Tables {
    let builtin = |native| {
        catalog
            .functions()
            .iter()
            .position(|f| Native::at(f.evaluate()) == Some(native))
            .unwrap() as u16
    };
    let ai_query = catalog
        .functions()
        .iter()
        .position(|f| f.evaluate() == AI_QUERY)
        .unwrap() as u16;
    let boolean = HsType::BOOLEAN.0;
    let void = HsType::VOID.0;
    let mut t = Tables::new();
    let yes = t.value(boolean, 1);
    let initial = t.call(builtin(Native::Not), "not", boolean, &[yes]);
    t.global_definition("sample", boolean, initial);
    let initial = t.global(0, "sample", boolean);
    t.global_definition("captured", boolean, initial);

    // Slot 0 sleeps inside begin, then wakes slot 1 in the same update.
    let duration = t.value(HsType::SHORT.0, 2);
    let sleep = t.call(builtin(Native::Sleep), "sleep", void, &[duration]);
    let target = t.literal(HsType::SCRIPT.0, "capture", 1);
    let wake = t.call(builtin(Native::Wake), "wake", void, &[target]);
    let root = t.call(builtin(Native::Begin), "begin", void, &[sleep, wake]);
    t.script("driver", 0, void, root);

    let dst = t.global(1, "captured", boolean);
    let src = t.global(0, "sample", boolean);
    let capture = t.call(builtin(Native::Set), "set", void, &[dst, src]);
    t.script("capture", 1, void, capture);

    // The continuous thread calls a static script containing a real
    // engine-facing query nested inside a fixed-signature boolean builtin.
    let dst = t.global(0, "sample", boolean);
    let value = t.script_call(3, "sense", boolean, &[]);
    let sample = t.call(builtin(Native::Set), "set", void, &[dst, value]);
    t.script("sampling", 2, void, sample);
    let read = t.call(ai_query, "read_ai", boolean, &[]);
    let sense = t.call(builtin(Native::Not), "not", boolean, &[read]);
    t.script("sense", 3, boolean, sense);
    t
}

#[derive(Default)]
struct Trace {
    builtins: BTreeMap<u16, u64>,
    dispatches: u64,
    thread_executions: u64,
    engine_calls: Vec<(i32, u16)>,
}

struct Oracle {
    code: Original,
    ram: FlatMemory,
    trace: Trace,
}

impl Oracle {
    fn new(image: &Image, tables: &Tables) -> Self {
        let mut arena = guest_layout::FlatMemory::new(ARENA, 0x20000);
        arena.write(ARENA, b"hs fixture\0").unwrap();
        let external = hs_pool::EXTERNALS as i32;
        for (addr, count, width) in [
            (THREADS, 4, 0x418),
            (GLOBALS, external + 2, 8),
            (LISTS, 1, 12),
        ] {
            let pool = DatumArray::at(addr);
            pool.initialize_contiguous(&mut arena, ARENA, count, width, 0, 0)
                .unwrap();
            pool.make_valid(&mut arena).unwrap();
        }
        for _ in 0..external {
            DatumArray::at(GLOBALS)
                .new_datum(&mut arena)
                .unwrap()
                .unwrap();
        }
        let mut payload = vec![0; 0x1800];
        tables.write(&mut payload, SCENARIO, 0x300);
        arena.write(SCENARIO, &payload).unwrap();
        let nodes = DatumArray::at(NODES);
        let count = arena.u32(SCENARIO + 0x238).unwrap() as i32;
        let data = arena.u32(SCENARIO + 0x23c).unwrap();
        nodes
            .initialize(
                &mut arena,
                ARENA,
                count,
                hs::Node::SIZE as i32,
                0,
                0,
                NODES + layout::HEADER_SIZE,
            )
            .unwrap();
        nodes.attach(&mut arena, count, data).unwrap();
        arena.set_u16(TIME + 2, 30).unwrap();
        arena.set_u8(GAME + 0x1120, 1).unwrap();

        let mut ram = image.ram.clone();
        assert!(ram.load(ARENA, &arena.bytes));
        for (addr, value) in [
            (SCENARIO_INDEX, 0),
            (points::SCENARIO.base, SCENARIO),
            (points::GAME_TIME.base, TIME),
            (points::GAME_GLOBALS.base, GAME),
            (hs_pool::THREADS, THREADS),
            (hs_pool::GLOBALS, GLOBALS),
            (hs_syntax::NODES, NODES),
            (OBJECT_LISTS, LISTS),
            (AI_GLOBALS, AI),
        ] {
            assert!(ram.load(addr, &value.to_le_bytes()));
        }
        Self {
            code: image.original(),
            ram,
            trace: Trace::default(),
        }
    }

    fn call(&mut self, entry: u32) {
        let mut cpu = Cpu {
            eip: entry,
            ..Cpu::default()
        };
        cpu.gpr[4] = STACK;
        assert!(self.ram.load(STACK, &HARNESS_EXIT.to_le_bytes()));
        for _ in 0..100_000 {
            match cpu.eip {
                HARNESS_EXIT => {
                    assert_eq!(cpu.gpr[4], STACK + 4);
                    return;
                }
                pc if pc == points::HS_THREAD_EXECUTE.base => self.trace.thread_executions += 1,
                DISPATCH_BUILTIN => {
                    self.trace.dispatches += 1;
                    let sp = cpu.gpr[4];
                    if self.ram.read_u32(sp + 8).unwrap() as u8 != 0 {
                        let index = self.ram.read_u32(sp).unwrap() as u16;
                        *self.trace.builtins.entry(index).or_default() += 1;
                    }
                }
                DISPATCH_SCRIPT => self.trace.dispatches += 1,
                AI_QUERY => {
                    let tick = self.ram.read_u32(TIME + 8).unwrap() as i32;
                    let index = self.ram.read_u32(cpu.gpr[4] + 4).unwrap() as u16;
                    self.trace.engine_calls.push((tick, index));
                }
                _ => {}
            }
            lifted::code::step(&mut self.code, &mut cpu, &mut self.ram).unwrap();
        }
        panic!(
            "guest entry {entry:#x} exceeded step budget at {:#x}",
            cpu.eip
        );
    }

    fn compare(&self, vm: &Vm<'_>, engine: &RecordingEngine) {
        let word = |a| self.ram.read_u32(a).unwrap();
        let threads = word(THREADS + layout::DATA);
        assert_eq!(
            word(THREADS + layout::ACTIVE_COUNT) as usize,
            vm.threads().count()
        );
        for (slot, thread) in vm.threads() {
            let record = threads + slot as u32 * 0x418;
            let header = word(record);
            assert_eq!(header as u16, thread.salt(), "slot {slot} salt");
            assert_eq!((header >> 16) as u8, 0, "slot {slot} script category");
            assert_eq!(Some(word(record + 4) as u16), thread.script());
            let wake = word(record + 8) as i32;
            let state = match wake {
                -1 => ThreadState::Finished,
                -2 => ThreadState::Dormant,
                w if w >= 0 => ThreadState::Waiting(w),
                w => ThreadState::Suspended(w),
            };
            assert_eq!(state, thread.state(), "slot {slot} at tick {}", engine.time);
            let root = record + 0x18;
            let mut frame = word(record + 0x10);
            let mut depth = 0;
            while frame != root {
                assert!(frame > root && frame < record + 0x418);
                let parent = word(frame);
                assert!(parent < frame);
                frame = parent;
                depth += 1;
            }
            assert_eq!(depth, thread.depth(), "slot {slot} live frames");
        }
        let cells = word(GLOBALS + layout::DATA) + hs_pool::EXTERNALS * 8;
        for index in 0..vm.scenario().globals().len() {
            let bits = word(cells + index as u32 * 8 + 4);
            assert_eq!(
                Some(Cell(bits)),
                vm.global(index),
                "global {index} at tick {}",
                engine.time
            );
        }
        assert!(vm.faults().is_empty(), "{:?}", vm.faults());
        assert_eq!(self.trace.builtins, vm.stats().builtins);
        assert_eq!(self.trace.dispatches, vm.stats().dispatches);
        assert_eq!(self.trace.thread_executions, vm.stats().thread_executions);
        assert_eq!(
            self.trace.engine_calls,
            engine
                .log
                .iter()
                .map(|c| (c.tick, c.index))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn lifted_map_init_and_ticks_match_the_vm_on_invented_scripts() {
    let Some(image) = Image::get() else { return };
    let catalog = Catalog::from_xbe(image.xbe).unwrap();
    let vm_image = VmImage::from_xbe(image.xbe, &catalog).unwrap();
    let tables = tables(&catalog);
    let scenario = tables.scenario();
    scenario.validate(&catalog).unwrap();
    let mut vm = Vm::new(&scenario, &catalog, &vm_image);
    let mut engine = RecordingEngine::new(Policy::Neutral);
    engine.keep = 16;
    let mut guest = Oracle::new(image, &tables);

    guest.call(CASTS_INITIALIZE);
    guest.call(points::HS_NEW_MAP.base);
    vm.initialize(&mut engine).unwrap();
    guest.compare(&vm, &engine);
    for tick in 0_i32..5 {
        engine.time = tick;
        // At the wake tick, slot 1 observes the previous tick's value of
        // sample because it executes before the continuous thread.
        let enabled = tick % 2 == 0;
        engine.policy = if enabled {
            Policy::Truthy
        } else {
            Policy::Neutral
        };
        assert!(guest.ram.load(TIME + 8, &tick.to_le_bytes()));
        assert!(guest.ram.load(AI, &[enabled as u8]));
        guest.call(points::SCRIPT_UPDATE.base);
        vm.update(&mut engine);
        guest.compare(&vm, &engine);
        assert_eq!(vm.global(0), Some(Cell::from_bool(!enabled)));
        assert_eq!(vm.global(1), Some(Cell::from_bool(tick >= 2)));
    }
    assert_eq!(engine.log.len(), 5);
}
