//! Compare each sound body to the owner's lifted function. Both sides start with
//! the same synthetic sound records. At every call we compare the complete live
//! frame (including the arguments a DirectSound callback would see), apply the
//! same fixture reply, and resume through the two independent implementations.
#![cfg(feature = "differential")]

use engine::sim_objects::{self, Body, Table};
use ir::interp::{Cpu, Flow, MemFault};
use lifted::Code;
use lifted::cpu::{from_interp, to_interp};
use lifted::image::Image;
use runtime::{Guest, TransferAbi, abi, guest::Permissions};

const RAM: usize = 8 << 20;
const ARENA: u32 = 0x0060_0000;
const STACK: u32 = 0x007f_0000;
const STACK_BOTTOM: u32 = STACK - 0x10000;
const EXIT: u32 = 0xffff_fffc;
const STACK_PROBE: u32 = 0x320560;
const FLOAT_TO_INTEGER: u32 = 0x322b0c;

struct Case {
    setup: fn(&mut Guest, &mut Cpu, u32),
    reply: fn(&mut Guest, &mut Cpu, u32, u32) -> u32,
    seed: u32,
}

struct Memory<'a>(&'a mut Guest);

impl ir::interp::Memory for Memory<'_> {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.0.read_bytes(addr, buf, 1).map_err(|f| MemFault {
            addr: f.addr,
            write: false,
        })
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        self.0.write_bytes(addr, data, 1).map_err(|f| MemFault {
            addr: f.addr,
            write: true,
        })
    }
}

fn start(image: &Image, entry: u32, case: &Case) -> (Guest, Cpu) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    assert_eq!(image.ram.base, 0);
    let len = image.ram.bytes.len().min(RAM);
    g.write_bytes(0, &image.ram.bytes[..len], 1).unwrap();
    // Dirty the whole potential stack frame, so missing initialization is visible.
    g.write_bytes(STACK_BOTTOM, &[0xa5; 65536], 1).unwrap();
    g.set_u32(STACK, EXIT).unwrap();
    let mut cpu = Cpu {
        eip: entry,
        gpr: [0x19, 0x321, 0x54, 0x765, STACK, 0x98, 0xba, 0xdc],
        eflags: 0x893,
        xmm: std::array::from_fn(|i| ((11 + i as u128) << 80) | 1),
        ..Cpu::default()
    };
    (case.setup)(&mut g, &mut cpu, case.seed);
    (g, cpu)
}

fn segment(code: &mut lifted::Original, cpu: &mut Cpu, g: &mut Guest) -> TransferAbi {
    for _ in 0..250_000 {
        match lifted::code::step(code, cpu, &mut Memory(g)).unwrap() {
            Flow::Call {
                target,
                continuation,
            } => return TransferAbi::call(target, continuation),
            Flow::Return { target } => return TransferAbi::ret(target),
            Flow::Jump { target, tail: true } => return TransferAbi::jump(target),
            _ => {}
        }
    }
    panic!("lifted sound segment did not return at {:#x}", cpu.eip);
}

fn answer(code: &mut lifted::Original, case: &Case, cpu: &mut Cpu, g: &mut Guest) {
    let sp = cpu.gpr[4];
    let site = g.u32(sp).unwrap();
    let target = cpu.eip;
    if matches!(target, STACK_PROBE | FLOAT_TO_INTEGER) {
        // The probe moves the caller's stack. The conversion pops an extended
        // float and touches x87 bookkeeping. Use the actual CRT for both arms.
        for _ in 0..10_000 {
            if let Flow::Return { target } = lifted::code::step(code, cpu, &mut Memory(g)).unwrap()
                && target == site
            {
                return;
            }
        }
        panic!("CRT call at {target:#x} did not return to {site:#x}");
    }
    let pop = (case.reply)(g, cpu, target, case.seed);
    cpu.gpr[4] = sp + 4 + pop;
    cpu.eip = site;
}

fn compare_ram(reference: &Guest, native: &Guest, esp: u32, context: &str) {
    fn bytes(g: &Guest) -> &[u8] {
        let start = g.mappings()[0].physical_start as usize;
        &g.physical_ram()[start..start + RAM]
    }
    let (a, b) = (bytes(reference), bytes(native));
    // The caller owns the stack above ESP. Below ESP earlier calls may
    // have used different transient scratch on the way to this boundary.
    let live_stack = esp.clamp(STACK_BOTTOM, STACK) as usize;
    for range in [0..STACK_BOTTOM as usize, live_stack..RAM] {
        if a[range.clone()] != b[range.clone()] {
            let at = range.into_iter().find(|&i| a[i] != b[i]).unwrap();
            panic!(
                "{context}: RAM at {at:#x}: lifted {:#04x}, native {:#04x}",
                a[at], b[at]
            );
        }
    }
}

fn compare(image: &Image, code: &mut lifted::Original, body: &'static Body, case: &Case) {
    compare_with(image, code, body, case, |_, _| {});
}

fn compare_with(
    image: &Image,
    code: &mut lifted::Original,
    body: &'static Body,
    case: &Case,
    dress: impl Fn(&mut Guest, &mut Cpu),
) {
    let root = body.point.base;
    // Isolate the entry: even a child with its own port must appear as a
    // callout here, with the same args and return site as the lifted code.
    let table = Table::new(std::iter::once(body)).unwrap();
    let (mut reference_ram, mut reference) = start(image, root, case);
    let (mut native_ram, mut native) = start(image, root, case);
    dress(&mut reference_ram, &mut reference);
    dress(&mut native_ram, &mut native);
    code.prime(root);
    for n in 0..8192 {
        let pc = reference.eip;
        let context = format!(
            "{} seed {} transfer {n} from {pc:#x}",
            body.point.name, case.seed
        );
        assert_eq!(native.eip, pc, "{context}");
        let a = segment(code, &mut reference, &mut reference_ram);
        let mut abi_cpu = from_interp(&native);
        let b = sim_objects::drive(&mut abi_cpu, &mut native_ram, &table, pc).expect(&context);
        assert_eq!(b, a, "{context}");
        native = to_interp(&abi_cpu);
        native.eip = b.target_pc;
        let diff = lifted::cpu::diff(&native, &reference);
        assert!(diff.is_empty(), "{context}: {diff:?}");
        compare_ram(&reference_ram, &native_ram, reference.gpr[4], &context);
        match a.kind {
            abi::RETURN => {
                assert_eq!(a.target_pc, EXIT, "{context}");
                return;
            }
            abi::CALL | abi::JUMP => {
                if engine::sim_sound::generated_sites(root).contains(&a.resume_pc) {
                    // This call hands its frame back to the image. Its return
                    // address, every live stack byte and register matched above.
                    return;
                }
                answer(code, case, &mut reference, &mut reference_ram);
                answer(code, case, &mut native, &mut native_ram);
                if a.kind == abi::JUMP {
                    assert_eq!(reference.eip, EXIT, "{context}: tail return");
                    assert_eq!(native.eip, EXIT, "{context}: native tail return");
                    return;
                }
            }
            _ => panic!("{context}: unexpected transfer {a:?}"),
        }
    }
    panic!("{} case {} did not finish", body.point.name, case.seed);
}

#[path = "sim_sound_differential/ambience.rs"]
mod ambience;
#[path = "sim_sound_differential/channel.rs"]
mod channel;
#[path = "sim_sound_differential/channel_choice.rs"]
mod channel_choice;
#[path = "sim_sound_differential/core.rs"]
mod core_cases;
#[path = "sim_sound_differential/disc.rs"]
mod disc;
#[path = "sim_sound_differential/extra.rs"]
mod extra;
#[path = "sim_sound_differential/impulse.rs"]
mod impulse;
#[path = "sim_sound_differential/gate.rs"]
mod start_gate;
#[path = "sim_sound_differential/levels.rs"]
mod levels;
#[path = "sim_sound_differential/listener.rs"]
mod listener;
#[path = "sim_sound_differential/loops.rs"]
mod loops;
#[path = "sim_sound_differential/objectsound.rs"]
mod objectsound;
#[path = "sim_sound_differential/portal_query.rs"]
mod portal_query;
#[path = "sim_sound_differential/scheduled_start.rs"]
mod scheduled_start;
#[path = "sim_sound_differential/segments.rs"]
mod segments;
#[path = "sim_sound_differential/sources.rs"]
mod sources;
#[path = "sim_sound_differential/spatial.rs"]
mod spatial;
#[path = "sim_sound_differential/stream.rs"]
mod stream;
#[path = "sim_sound_differential/streammix.rs"]
mod streammix;
#[path = "sim_sound_differential/table.rs"]
mod table;

fn cases(entry: u32) -> Vec<Case> {
    let providers: &[fn(u32) -> Vec<Case>] = &[
        ambience::cases,
        channel::cases,
        channel_choice::cases,
        core_cases::cases,
        extra::cases,
        impulse::cases,
        listener::cases,
        loops::cases,
        objectsound::cases,
        portal_query::cases,
        scheduled_start::cases,
        segments::cases,
        sources::cases,
        spatial::cases,
        stream::cases,
        streammix::cases,
    ];
    providers
        .iter()
        .flat_map(|provider| provider(entry))
        .collect()
}

#[test]
fn every_installed_sound_body_has_a_fixture() {
    let table = &*engine::sim_sound::TABLE;
    assert!(!table.is_empty());
    for &body in table.bodies() {
        assert!(!cases(body.point.base).is_empty(), "{}", body.point.name);
    }
}

#[test]
fn native_sound_matches_lifted_at_each_call_and_return() {
    let Some(image) = Image::get() else { return };
    let mut code = image.original();
    for &body in engine::sim_sound::TABLE.bodies() {
        for case in cases(body.point.base) {
            compare(image, &mut code, body, &case);
        }
    }
}
