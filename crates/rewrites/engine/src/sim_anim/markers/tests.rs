//! The adapter against the kernel called by hand on the same fixture
//! (synthetic guest memory, no image): the port must deliver the same
//! records, the count and the continuation, and a decline must leave the
//! port exactly as it is with the kernel off. The port itself, and with
//! it this path, is held equal to the lifted guest by
//! `tests/marker_differential.rs`.
use super::*;
use crate::points::{OBJECTS, TAG_ENTRIES};
use runtime::Guest;
use runtime::guest::Permissions;
use std::sync::Mutex;

/// The kernel switch is process-global; the tests that flip it serialise
/// on this lock so none of them runs another's setting.
static SWITCH: Mutex<()> = Mutex::new(());

const RAM: u32 = 0x0080_0000;
const STACK: u32 = 0x007f_0000;
const HEADER: u32 = 0x0050_0000;
const SLOTS: u32 = 0x0050_1000;
const OBJECT: u32 = 0x0060_0000;
const TAGS: u32 = 0x0061_0000;
const DEF: u32 = 0x0061_1000;
const MODEL_A: u32 = 0x0061_2000;
const RENDER: u32 = 0x0061_3000;
const GROUPS_AT: u32 = 0x0061_4000;
const INSTANCES_AT: u32 = 0x0061_5000;
/// `OBJECT + 0x4000` and `OBJECT + 0x6000`: the object's node and matrix
/// arrays, named by signed 16-bit offsets.
const NODE_ARRAY: u32 = OBJECT + 0x4000;
const MATRICES: u32 = OBJECT + 0x6000;
const OUT: u32 = 0x0063_0000;
const HANDLE: u32 = 4;
const DEF_HANDLE: u32 = 1;
const MODEL_HANDLE: u32 = 2;
const RENDER_HANDLE: u32 = 3;
const NAME: u32 = 0x2a;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// A magnitude in `[0.25, 4)`, either sign.
    fn value(&mut self) -> u32 {
        let r = self.next();
        let sign = ((r >> 40) as u32 & 1) << 31;
        sign | (125 + (r % 4) as u32) << 23 | (r >> 8) as u32 & 0x7f_ffff
    }
}

/// One instance, `0x24` bytes.
fn instance(node: u8, name: u8, out: u8, t: [f32; 3], q: [f32; 4], extra: f32) -> Vec<u8> {
    let mut b = vec![node, name, out, 0];
    for v in t {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for v in q {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&extra.to_le_bytes());
    assert_eq!(b.len(), ::sim_anim::markers::INSTANCE_BYTES);
    b
}

/// The synthetic world: one object whose definition names a model whose
/// render model holds the marker groups.
#[derive(Clone)]
struct World {
    instances: Vec<u8>,
    matrices: Vec<NodeMatrix>,
    mirror: bool,
    groups: u32,
    /// The name's group, or the count for "absent".
    found: Option<usize>,
    capacity: i32,
    seed: u64,
}

fn world(seed: u64) -> World {
    World {
        instances: Vec::new(),
        matrices: Vec::new(),
        mirror: false,
        groups: 2,
        found: Some(1),
        capacity: 16,
        seed,
    }
}

fn machine(w: &World) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    let mut rng = Rng(w.seed * 0x9e37_79b9 + 1);
    let word = |g: &mut Guest, a: u32, v: u32| g.set_u32(a, v).unwrap();

    // The divide-by-0x34 magic the port reads out of its own instruction
    // stream at `0x0b8db7`: `floor(2^36 / 0x34) + 1`.
    word(&mut g, 0x000b_8db7, ((1u64 << 36) / 0x34 + 1) as u32);
    word(&mut g, ONE, 1.0f32.to_bits());
    word(&mut g, SMALL, 1.0e-4f32.to_bits());
    word(&mut g, TWO, 2.0f32.to_bits());

    word(&mut g, OBJECTS.base, HEADER);
    word(&mut g, HEADER + 0x44, SLOTS);
    word(&mut g, SLOTS + 12 * HANDLE + 8, OBJECT);
    word(&mut g, TAG_ENTRIES.base, TAGS);
    for (h, data) in [
        (DEF_HANDLE, DEF),
        (MODEL_HANDLE, MODEL_A),
        (RENDER_HANDLE, RENDER),
    ] {
        word(&mut g, TAGS + 16 * h + 8, data);
    }

    word(&mut g, OBJECT, DEF_HANDLE);
    word(&mut g, OBJECT + 4, if w.mirror { 1 << 10 } else { 0 });
    word(&mut g, OBJECT + 0x14, u32::MAX);
    g.set_u16(
        OBJECT + 0x114,
        (0x34 * w.matrices.len().max(1) as u32) as u16,
    )
    .unwrap();
    g.set_u16(OBJECT + 0x116, (MATRICES - OBJECT) as u16)
        .unwrap();
    g.set_u16(OBJECT + 0x11a, (NODE_ARRAY - OBJECT) as u16)
        .unwrap();
    word(&mut g, DEF + 0x38, MODEL_HANDLE);
    word(&mut g, MODEL_A + 4, RENDER_HANDLE);
    word(&mut g, RENDER + 0x58, w.groups);
    word(&mut g, RENDER + 0x5c, GROUPS_AT);

    // The groups: the wanted name sits in the second.
    let count = w.instances.len() / ::sim_anim::markers::INSTANCE_BYTES;
    for i in 0..w.groups as usize {
        let group = GROUPS_AT + 12 * i as u32;
        let name = if w.found == Some(i) {
            NAME
        } else {
            NAME + 1 + i as u32
        };
        let holds = w.found == Some(i);
        word(&mut g, group, name);
        word(&mut g, group + 4, if holds { count as u32 } else { 0 });
        word(&mut g, group + 8, if holds { INSTANCES_AT } else { 0 });
    }
    if !w.instances.is_empty() {
        g.write_bytes(INSTANCES_AT, &w.instances, 1).unwrap();
    }
    // The node array's name bytes: every node answers its own index.
    for i in 0..8u8 {
        g.set_u8(NODE_ARRAY + i as u32, 0x40 + i).unwrap();
    }
    for (i, m) in w.matrices.iter().enumerate() {
        g.write_bytes(MATRICES + 0x34 * i as u32, &m.to_bytes(), 1)
            .unwrap();
    }

    word(&mut g, STACK, 0x7000_0000);
    word(&mut g, STACK + 4, HANDLE);
    word(&mut g, STACK + 8, NAME);
    word(&mut g, STACK + 0xc, w.capacity as u32);
    let mut c = CpuAbi {
        eax: rng.next() as u32,
        ecx: 1, // `cl` set: no parent resolution
        edx: rng.next() as u32,
        ebx: OUT,
        esp: STACK,
        ebp: rng.next() as u32,
        esi: rng.next() as u32,
        edi: rng.next() as u32,
        eflags: 0x246,
        eflags_defined: 0x8d5,
        mxcsr: 0x1fa0,
        ..CpuAbi::default()
    };
    for reg in c.xmm.iter_mut() {
        let v = rng.value();
        *reg = v.to_le_bytes().repeat(4).try_into().unwrap();
    }
    (c, g)
}

/// A populated world: four matrices, five instances, one filtered out.
fn populated(seed: u64) -> World {
    let mut rng = Rng(seed ^ 0x5bd1_e995);
    let mut w = world(seed);
    for _ in 0..4 {
        let mut m = NodeMatrix::default();
        m.scale = rng.value();
        for x in m
            .forward
            .0
            .iter_mut()
            .chain(&mut m.left.0)
            .chain(&mut m.up.0)
            .chain(&mut m.position.0)
        {
            *x = rng.value();
        }
        w.matrices.push(m);
    }
    let quat =
        |rng: &mut Rng| -> [f32; 4] { std::array::from_fn(|_| f32::from_bits(rng.value()) * 0.25) };
    let at = |rng: &mut Rng| -> [f32; 3] { std::array::from_fn(|_| f32::from_bits(rng.value())) };
    for (node, name) in [(0u8, 0x40u8), (1, 0x41), (2, 0x42), (0xff, 9), (3, 0x99)] {
        let out = [0u8, 1, 2, 3][(node & 3) as usize];
        let q = quat(&mut rng);
        let t = at(&mut rng);
        w.instances
            .extend(instance(node, name, out, t, q, f32::from_bits(rng.value())));
    }
    // Node 3 names a byte the node array does not answer: filtered out.
    w
}



/// The records the kernel itself builds from the same world.
fn reference(w: &World) -> Vec<Marker> {
    // No group to scan, or no group answering the name: nothing selected.
    if w.groups == 0 || w.found.is_none() {
        return Vec::new();
    }
    let mut window = [0u8; 256];
    for i in 0..8u8 {
        window[128 + i as usize] = 0x40 + i;
    }
    let k = Constants {
        one: 1.0f32.to_bits(),
        small: 1.0e-4f32.to_bits(),
        two: 2.0f32.to_bits(),
    };
    let sel = ::sim_anim::markers::select(&w.instances, Some(&window), w.capacity);
    ::sim_anim::markers::model_markers(&k, &w.instances, &sel, &w.matrices, w.mirror).unwrap()
}

/// Run the port with the kernel on; the port must hand back the records,
/// the count and the continuation at `b8ded`.
fn fills(w: &World) {
    let _guard = SWITCH.lock().unwrap();
    set_enabled(true);
    let expected = reference(w);
    let (mut cpu, mut g) = machine(w);
    let transfer = crate::sim_native::marker::transforms(&mut cpu, &mut g).unwrap();
    set_enabled(false);
    assert_eq!(transfer, TransferAbi::resume(AFTER_MARKERS));
    assert_eq!(cpu.eax, expected.len() as u32);
    // The guest call leaves the stack at `b8d30`'s own frame, and the
    // callee-saved registers as the port set them for the call.
    assert_eq!(cpu.esp, STACK - 0x14);
    assert_eq!(cpu.ebx, OUT);
    assert_eq!(cpu.ebp, MATRICES);
    assert_eq!(cpu.esi, NODE_ARRAY);
    assert_eq!(cpu.edi, NAME);
    for (n, m) in expected.iter().enumerate() {
        let at = OUT + ::sim_anim::markers::MARKER_BYTES as u32 * n as u32;
        let mut bytes = [0u8; ::sim_anim::markers::MARKER_BYTES];
        g.read_bytes(at, &mut bytes, 1).unwrap();
        assert_eq!(bytes.as_slice(), m.to_bytes().as_slice(), "marker {n}");
    }
}

#[test]
fn the_port_fills_the_markers_through_the_kernel() {
    fills(&populated(1));
}

#[test]
fn a_fresh_mxcsr_fills_the_same_markers_and_sets_precision() {
    // 03a_oldmombasa: `anim marker: float`. Retail's SSE runs whatever the
    // MXCSR holds; here PE is not yet sticky, as after the guest's own
    // `ldmxcsr`. The records are the sticky run's and PE comes out set.
    let _guard = SWITCH.lock().unwrap();
    set_enabled(true);
    let w = populated(1);
    let expected = reference(&w);
    assert!(!expected.is_empty());
    let (mut cpu, mut g) = machine(&w);
    cpu.mxcsr = 0x1f80;
    let transfer = crate::sim_native::marker::transforms(&mut cpu, &mut g).unwrap();
    set_enabled(false);
    assert_eq!(transfer, TransferAbi::resume(AFTER_MARKERS));
    assert_eq!(cpu.mxcsr, 0x1fa0);
    for (n, m) in expected.iter().enumerate() {
        let at = OUT + ::sim_anim::markers::MARKER_BYTES as u32 * n as u32;
        let mut bytes = [0u8; ::sim_anim::markers::MARKER_BYTES];
        g.read_bytes(at, &mut bytes, 1).unwrap();
        assert_eq!(bytes.as_slice(), m.to_bytes().as_slice(), "marker {n}");
    }
}

#[test]
fn a_mirrored_object_negates_the_world_left_column() {
    let mut w = populated(2);
    w.mirror = true;
    fills(&w);
    let plain = reference(&populated(2));
    let mirrored = reference(&w);
    assert_ne!(plain, mirrored);
    assert_eq!(mirrored.len(), plain.len());
}

#[test]
fn a_full_output_stops_at_the_capacity() {
    let mut w = populated(3);
    w.capacity = 2;
    fills(&w);
    assert_eq!(reference(&w).len(), 2);
}

#[test]
fn a_name_no_group_answers_fills_nothing() {
    let mut w = populated(4);
    w.found = None;
    fills(&w);
}

#[test]
fn a_model_without_groups_fills_nothing() {
    let mut w = populated(5);
    w.groups = 0;
    fills(&w);
}

/// A declined call panics `anim marker: <reason>` at the port; no guest
/// `1d8f50` call runs.
fn declines(w: &World, edit: impl Fn(&mut CpuAbi, &mut Guest), why: Decline) {
    let _guard = SWITCH.lock().unwrap();
    set_enabled(true);
    let (mut cpu, mut g) = machine(w);
    edit(&mut cpu, &mut g);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = crate::sim_native::marker::transforms(&mut cpu, &mut g);
    }));
    set_enabled(false);
    let message = panic.expect_err("a decline panics").downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.ends_with(&format!("anim marker: {}", why.name())), "{why:?}: {message}");
}

const MODEL_MARKERS: u32 = 0x001d_8f50;

#[test]
fn calls_off_the_fast_path_decline_untouched() {
    let w = populated(6);
    // PE not yet sticky with precision unmasked: the first inexact
    // operation would raise `#XM`.
    declines(
        &World {
            seed: 6,
            ..w.clone()
        },
        |c, _| c.mxcsr = 0x0f80,
        Decline::Unmasked,
    );
    // More groups than the scan bound, and none matches.
    let mut wide = populated(7);
    wide.groups = GROUPS_MAX as u32 + 4;
    wide.found = None;
    declines(&wide, |_, _| {}, Decline::Scan);
    // More instances than the kernel gathers.
    let huge = populated(8);
    let count = INSTANCES_MAX as u32 + 1;
    declines(
        &World {
            seed: 8,
            ..huge.clone()
        },
        move |_, g| g.set_u32(GROUPS_AT + 12 + 4, count).unwrap(),
        Decline::Count,
    );
    // An instance pointer past the end of RAM.
    declines(
        &w,
        |_, g| g.set_u32(GROUPS_AT + 12 + 8, RAM + 0x1000).unwrap(),
        Decline::Fault,
    );
    // The output over the instances.
    declines(&w, |c, _| c.ebx = INSTANCES_AT + 8, Decline::Alias);
}

#[test]
fn the_switch_turns_the_kernel_off() {
    let _guard = SWITCH.lock().unwrap();
    let w = populated(9);
    let (mut cpu, mut g) = machine(&w);
    set_enabled(false);
    let transfer = crate::sim_native::marker::transforms(&mut cpu, &mut g).unwrap();
    assert_eq!(transfer, TransferAbi::call(MODEL_MARKERS, AFTER_MARKERS));
    let was = stats().native;
    set_enabled(true);
    let (mut cpu, mut g) = machine(&w);
    crate::sim_native::marker::transforms(&mut cpu, &mut g).unwrap();
    set_enabled(false);
    assert_eq!(stats().native, was + 1);
}
