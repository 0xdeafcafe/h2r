//! The evaluator on synthetic memory: every curve kind, every transition
//! and periodic shape, each export flag, the base level's names, and the
//! paths `bab40` takes. No game data: the names, tables and constants are
//! made up, and the curve kernel reading guest memory directly is the
//! reference for the Rust-owned copy.

use super::*;
use std::collections::HashMap;

#[derive(Default)]
struct Ram(HashMap<u32, u8>);

impl Ram {
    fn set8(&mut self, at: u32, v: u8) {
        self.0.insert(at, v);
    }
    fn set16(&mut self, at: u32, v: u16) {
        for i in 0..2 {
            self.set8(at + i, (v >> (8 * i)) as u8);
        }
    }
    fn set32(&mut self, at: u32, v: u32) {
        for i in 0..4 {
            self.set8(at + i, (v >> (8 * i)) as u8);
        }
    }
    fn setf(&mut self, at: u32, v: f32) {
        self.set32(at, v.to_bits());
    }
    fn zero(&mut self, at: u32, len: u32) {
        for i in 0..len {
            self.set8(at + i, 0);
        }
    }
}

impl Memory for Ram {
    fn u32(&self, at: u32) -> Option<u32> {
        let mut v = 0;
        for i in 0..4 {
            v |= u32::from(*self.0.get(&at.wrapping_add(i))?) << (8 * i);
        }
        Some(v)
    }
    fn u16(&self, at: u32) -> Option<u16> {
        Some(u16::from(self.u8(at)?) | u16::from(self.u8(at.wrapping_add(1))?) << 8)
    }
    fn u8(&self, at: u32) -> Option<u8> {
        self.0.get(&at).copied()
    }
}

// The synthetic map.
const OBJECTS_PTR: u32 = 0x100;
const TAGS_PTR: u32 = 0x104;
const WORLD_PTR: u32 = 0x108;
const GAME_PTR: u32 = 0x10c;
const TIME_PTR: u32 = 0x110;
const TYPES: u32 = 0x200;
const POOL: u32 = 0x1000;
const HEADERS: u32 = 0x2000;
const TAG_ENTRIES: u32 = 0x5000;
const WORLD: u32 = 0x9000;
const GAME: u32 = 0x9100;
const CLOCK: u32 = 0x9200;
const SHAPES_AT: u32 = 0x1_0000;
const WAVES_AT: u32 = 0x2_0000;
const BLOBS: u32 = 0x3_0000;
const REF: u32 = 0x4_0000;
const BASE_IMPORT: u32 = 0x000b_cc20;
const UNIT_IMPORT: u32 = 0x000c_6b90;

/// The object (slot 1), its parent (slot 2) and the null handle's slot.
const OBJECT: u32 = 0xabcd_0001;
const PARENT: u32 = 0xabce_0002;
const BODY: u32 = 0x3000;
const PARENT_BODY: u32 = 0x3400;
const NULL_BODY: u32 = 0x3800;
/// Tag slots: the object's definition, the parent's, the model.
const DEF_TAG: u32 = 3;
const PARENT_TAG: u32 = 5;
const MODEL_TAG: u32 = 4;
const DEF: u32 = 0x6000;
const PARENT_DEF: u32 = 0x6400;
const MODEL: u32 = 0x6800;
const RECORDS: u32 = 0x7000;
const PARENT_RECORDS: u32 = 0x7800;

const NAMES: BaseNames = BaseNames {
    one: 10,
    zero: 11,
    flag: 12,
    facing: 20,
    variant: 21,
    bcbb0: 22,
    bca70: 23,
    bcad0: 30,
    one_b: 31,
    bcb10_delta: 1,
    body_ec: 40,
    component: 41,
    body_f0: 42,
    random: 50,
    shield_test: 51,
    one_c: 52,
    body_f8: 53,
    c18e0: 60,
    body_f4: 61,
    component_lo: 62,
    component_hi: 64,
    f0_less_one: 70,
    component_b: 71,
    world_70_delta: 1,
    world_78: 80,
    world_7c: 81,
    component_lo_b: 82,
    component_hi_b: 84,
};

const NAME_ONE: u32 = 1000;
const NAME_ZERO: u32 = 1001;
/// Export names.
const E1: u32 = 2000;
const E2: u32 = 2001;
const E3: u32 = 2002;
/// A name no level knows.
const UNKNOWN: u32 = 13;

const WAVE_SCALE: f32 = 1024.0 / 28.0;

const IMAGE: Image = Image {
    one: 1.0f32.to_bits(),
    shape_scale: 1023.0f32.to_bits(),
    shape_bias: 0.1f32.to_bits(),
    wave_scale: WAVE_SCALE.to_bits(),
    byte: (1.0f32 / 255.0).to_bits(),
    wrap_high: 0.75f32.to_bits(),
    wrap_low: 0.25f32.to_bits(),
    shapes: 0,
    waves: 0,
};

const TIME_SCALE: f32 = 1.0 / 65536.0;
const RANDOM_SCALE: f32 = 1.0 / 65535.0;

fn constants() -> Constants {
    Constants {
        globals: Globals {
            objects: OBJECTS_PTR,
            tags: TAGS_PTR,
            types: TYPES,
            world: WORLD_PTR,
            game: GAME_PTR,
            game_time: TIME_PTR,
            base_import: BASE_IMPORT,
        },
        one: 1.0f32.to_bits(),
        zero: 0,
        name_one: NAME_ONE,
        name_zero: NAME_ZERO,
        time_hash: Hash {
            mul: 0x0019_660d,
            add: 0x3c6e_f35f,
        },
        time_scale: TIME_SCALE.to_bits(),
        random_hash: Hash {
            mul: 0x41c6_4e6d,
            add: 0x3039,
        },
        random_scale: RANDOM_SCALE.to_bits(),
        base: NAMES,
        curve: IMAGE,
    }
}

const ENV: Env = Env {
    mxcsr: 0x1fa0,
    x87_control: 0x027f,
    x87_ready: true,
};

/// A shape or wave table byte: distinct per sub-type, with a sawtooth
/// whose wrap the sliding waves (6 and 7) cross.
fn table_byte(sub: u32, i: u32) -> u8 {
    (((i + 37 * sub) & 0x3ff) / 4) as u8
}

/// The map: pointers, the pool, tags, two type descriptors (0: the base
/// level only; 1: base then a unit level), the tables in "image" memory.
fn world() -> (Ram, Library) {
    let mut ram = Ram::default();
    ram.set32(OBJECTS_PTR, POOL);
    ram.set32(POOL + 0x44, HEADERS);
    ram.set32(TAGS_PTR, TAG_ENTRIES);
    ram.set32(WORLD_PTR, WORLD);
    ram.set32(GAME_PTR, GAME);
    ram.set32(TIME_PTR, CLOCK);
    for (slot, body, tag) in [
        (1u32, BODY, DEF_TAG),
        (2, PARENT_BODY, PARENT_TAG),
        (0xffff, NULL_BODY, DEF_TAG),
    ] {
        ram.set32(HEADERS + slot * 12 + 8, body);
        ram.zero(body, 0x200);
        ram.set32(body, 0xe000_0000 | tag);
        ram.set32(body + 0x14, NONE);
        ram.set32(body + 0x13c, NONE);
    }
    for (tag, def) in [(DEF_TAG, DEF), (PARENT_TAG, PARENT_DEF), (MODEL_TAG, MODEL)] {
        ram.set32(TAG_ENTRIES + tag * 16 + 8, def);
        ram.zero(def, 0x100);
    }
    ram.set32(DEF + 0x38, MODEL_TAG);
    ram.set32(DEF + 0x68, RECORDS);
    ram.set32(PARENT_DEF + 0x68, PARENT_RECORDS);
    ram.set32(MODEL + 0x50, 4);
    ram.zero(WORLD, 0x100);
    ram.zero(GAME, 0x10);
    ram.zero(CLOCK, 0x10);
    ram.setf(CLOCK + 4, 1.0 / 30.0);
    ram.set32(CLOCK + 8, 1234);
    // Type 0: [base]; type 1: [base, unit].
    ram.set32(TYPES, 0x8000);
    ram.set32(TYPES + 4, 0x8200);
    ram.set32(0x8084, 0x8100);
    ram.set32(0x8088, 0);
    ram.set32(0x8284, 0x8100);
    ram.set32(0x8288, 0x8300);
    ram.set32(0x828c, 0);
    ram.set32(0x8100 + 0x4c, BASE_IMPORT);
    ram.set32(0x8300 + 0x4c, UNIT_IMPORT);
    // The image's tables run on into memory past them, as the image's do.
    for sub in 0..(SHAPES as u32 + 2) {
        for i in 0..TABLE as u32 {
            ram.set8(SHAPES_AT + sub * 1024 + i, table_byte(sub + 100, i));
        }
    }
    for sub in 0..WAVES as u32 {
        for i in 0..TABLE as u32 {
            ram.set8(WAVES_AT + sub * 1024 + i, table_byte(sub, i));
        }
    }
    let tables = Tables::read(&ram, SHAPES_AT, WAVES_AT).unwrap();
    (ram, Library::new(constants(), tables))
}

/// A blob of `kind` at blob slot `n`: flags, the two sub-types, the
/// output range, and the params from `+14`.
fn blob(
    ram: &mut Ram,
    n: u32,
    kind: u8,
    flags: u8,
    sub: [u8; 2],
    range: [f32; 2],
    params: &[f32],
) -> u32 {
    let at = BLOBS + n * 0x100;
    ram.zero(at, 0x100);
    ram.set8(at, kind);
    ram.set8(at + 1, flags);
    ram.set8(at + 2, sub[0]);
    ram.set8(at + 3, sub[1]);
    ram.setf(at + 4, range[0]);
    ram.setf(at + 8, range[1]);
    for (i, &p) in params.iter().enumerate() {
        ram.setf(at + 0x14 + 4 * i as u32, p);
    }
    at
}

/// A blob in slot 0 and the reference at [`REF`] to it.
fn curve_at(ram: &mut Ram, kind: u8, flags: u8, sub: [u8; 2], range: [f32; 2], params: &[f32]) {
    let data = blob(ram, 0, kind, flags, sub, range, params);
    reference(ram, data);
}

/// The reference at [`REF`] to `data`.
fn reference(ram: &mut Ram, data: u32) {
    ram.set32(REF, 1);
    ram.set32(REF + 4, data);
}

/// Forty params, in range, different from each other.
fn pattern() -> Vec<f32> {
    (0..40)
        .map(|i| 0.125 * (i % 7) as f32 - 0.25 + 0.03125 * (i / 7) as f32)
        .collect()
}

/// `13bb90` on guest memory: the curve kernel reading the blob and the
/// "image" tables where they are, then the output range in plain `f32`.
fn guest_function(ram: &Ram, x: f32, r: f32) -> Result<u32, Decline> {
    let data = ram.u32(REF + 4).unwrap();
    if data == 0 || ram.u32(REF).unwrap() as i32 <= 0 {
        return Ok(0);
    }
    let image = Image {
        shapes: SHAPES_AT,
        waves: WAVES_AT,
        ..IMAGE
    };
    let v = match curve::evaluate(
        &image,
        ENV.mxcsr,
        ENV.x87_control,
        REF,
        x.to_bits(),
        r.to_bits(),
        |a| ram.u32(a).ok_or(()),
        |a| ram.u8(a).ok_or(()),
    ) {
        Ok(curve::Answer::Value(v)) => v,
        Ok(curve::Answer::Declined(curve::Decline::Float)) => return Err(Decline::Float),
        Ok(curve::Answer::Declined(curve::Decline::Kind)) | Err(()) => return Err(Decline::Curve),
    };
    if ram.u8(data + 1).unwrap() & 0xf0 != 0 {
        return Ok(v);
    }
    let v = f32::from_bits(v);
    let v = v.clamp(0.0, 1.0);
    let (lo, hi) = (
        f32::from_bits(ram.u32(data + 4).unwrap()),
        f32::from_bits(ram.u32(data + 8).unwrap()),
    );
    Ok(((hi - lo) * v + lo).to_bits())
}

/// `13bb90` over the Rust-owned copy of the reference at [`REF`].
fn rust_function(lib: &mut Library, ram: &Ram, x: f32, r: f32) -> Result<u32, Decline> {
    let f = Function::read(ram, REF).unwrap();
    let mut e = Eval {
        lib,
        m: ram,
        env: ENV,
        fp: Fp::new(),
        depth: 0,
    };
    let v = e.function(&f, x.to_bits(), r.to_bits())?;
    if !e.fp.exact() {
        return Err(Decline::Float);
    }
    Ok(v)
}

const XS: [f32; 13] = [
    -0.5, 0.0, 0.001, 0.1, 0.25, 0.3333, 0.5, 0.7, 0.9, 0.999, 1.0, 1.5, 3.75,
];
const RS: [f32; 3] = [0.0, 0.3, 1.0];

/// The Rust-owned evaluation equals the guest-memory one at every input;
/// returns how many inputs gave a value.
fn sweep(lib: &mut Library, ram: &Ram, what: &str) -> usize {
    let mut values = 0;
    for x in XS {
        for r in RS {
            let want = guest_function(ram, x, r);
            assert_eq!(rust_function(lib, ram, x, r), want, "{what} at x {x} r {r}");
            values += usize::from(want.is_ok());
        }
    }
    values
}

#[test]
fn every_curve_kind_matches_the_kernel_on_guest_memory() {
    let (mut ram, mut lib) = world();
    for kind in 0..=11u8 {
        for flags in [0u8, 1] {
            let data = blob(&mut ram, 0, kind, flags, [1, 2], [0.0, 1.0], &pattern());
            reference(&mut ram, data);
            let what = format!("kind {kind} flags {flags}");
            if kind == 9 || kind == 10 {
                assert_eq!(
                    rust_function(&mut lib, &ram, 0.5, 0.5),
                    Err(Decline::Curve),
                    "{what}"
                );
                continue;
            }
            assert!(
                sweep(&mut lib, &ram, &what) > 0,
                "{what}: no input gave a value"
            );
        }
    }
}

#[test]
fn the_simple_kinds_give_their_values() {
    let (mut ram, mut lib) = world();
    let v = |lib: &mut Library, ram: &Ram, x: f32, r: f32| {
        f32::from_bits(rust_function(lib, ram, x, r).unwrap())
    };
    // Identity, through an output range of 2..4.
    curve_at(&mut ram, 0, 0, [0, 0], [2.0, 4.0], &[]);
    assert_eq!(v(&mut lib, &ram, 0.25, 0.0), 2.5);
    assert_eq!(v(&mut lib, &ram, 1.5, 0.0), 4.0);
    // Constant: 0 unblended (the range's low end), the range input blended.
    curve_at(&mut ram, 1, 0, [0, 0], [0.0, 1.0], &[]);
    assert_eq!(v(&mut lib, &ram, 0.25, 0.75), 0.0);
    curve_at(&mut ram, 1, 1, [0, 0], [0.0, 1.0], &[]);
    assert_eq!(v(&mut lib, &ram, 0.25, 0.75), 0.75);
    // Linear: x * m + b, with m and b at params +10 and +14.
    curve_at(
        &mut ram,
        4,
        0,
        [0, 0],
        [0.0, 1.0],
        &[0.0, 0.0, 0.0, 0.0, 0.5, 0.25],
    );
    assert_eq!(v(&mut lib, &ram, 0.5, 0.0), 0.5);
    // Kinds 6, 8 and 11 are 0, mapped to the range's low end.
    for kind in [6u8, 8, 11] {
        curve_at(&mut ram, kind, 0, [0, 0], [0.5, 1.0], &[]);
        assert_eq!(v(&mut lib, &ram, 0.5, 0.0), 0.5, "kind {kind}");
    }
    // An empty reference is 0.
    ram.set32(REF, 0);
    assert_eq!(rust_function(&mut lib, &ram, 0.5, 0.0), Ok(0));
}

#[test]
fn the_output_range_maps_and_flags_f0_take_the_curve_raw() {
    let (mut ram, mut lib) = world();
    curve_at(&mut ram, 0, 0x10, [0, 0], [2.0, 4.0], &[]);
    assert_eq!(
        rust_function(&mut lib, &ram, 0.25, 0.0),
        Ok(0.25f32.to_bits())
    );
    curve_at(
        &mut ram,
        4,
        0,
        [0, 0],
        [-1.0, 1.0],
        &[0.0, 0.0, 0.0, 0.0, 4.0, 0.0],
    );
    // 4 * 0.5 = 2 clamps to 1 before the range: the top, 1.
    assert_eq!(
        rust_function(&mut lib, &ram, 0.5, 0.0),
        Ok(1.0f32.to_bits())
    );
    assert_eq!(sweep(&mut lib, &ram, "linear ranged"), XS.len() * RS.len());
}

#[test]
fn every_transition_shape() {
    let (mut ram, mut lib) = world();
    for sub in 0..SHAPES as u8 {
        for flags in [0u8, 1] {
            let data = blob(
                &mut ram,
                0,
                2,
                flags,
                [sub, (sub + 3) % SHAPES as u8],
                [0.0, 1.0],
                &[0.1, 0.9, 0.8, 0.2],
            );
            reference(&mut ram, data);
            let what = format!("transition {sub} flags {flags}");
            assert_eq!(sweep(&mut lib, &ram, &what), XS.len() * RS.len(), "{what}");
        }
    }
    // Sub-type 0 is the input itself: a + (b - a) * x.
    curve_at(&mut ram, 2, 0, [0, 0], [0.0, 1.0], &[0.25, 0.75]);
    assert_eq!(
        rust_function(&mut lib, &ram, 0.5, 0.0),
        Ok(0.5f32.to_bits())
    );
    // A sub-type past the copied tables declines.
    curve_at(&mut ram, 2, 0, [SHAPES as u8, 0], [0.0, 1.0], &[0.1, 0.9]);
    assert_eq!(rust_function(&mut lib, &ram, 0.5, 0.0), Err(Decline::Curve));
}

#[test]
fn every_periodic_shape_including_the_sliding_waves_wrap() {
    let (mut ram, mut lib) = world();
    for sub in 0..WAVES as u8 {
        for flags in [0u8, 1] {
            let params = [2.0, 0.25, 0.2, 0.7, 3.0, 0.1, 0.9, 0.3];
            let data = blob(
                &mut ram,
                0,
                3,
                flags,
                [sub, (sub + 5) % WAVES as u8],
                [0.0, 1.0],
                &params,
            );
            reference(&mut ram, data);
            let what = format!("periodic {sub} flags {flags}");
            assert_eq!(sweep(&mut lib, &ram, &what), XS.len() * RS.len(), "{what}");
        }
    }
    // Sub-type 0 is 1: c + (d - c), the top of the wave's range.
    curve_at(&mut ram, 3, 0, [0, 0], [0.0, 1.0], &[1.0, 0.0, 0.25, 0.75]);
    assert_eq!(
        rust_function(&mut lib, &ram, 0.3, 0.0),
        Ok(0.75f32.to_bits())
    );
    // The sliding waves wrap where the sawtooth drops: halfway between
    // the index whose byte is 255 and the next, 0 (i + 37 * sub = 1023).
    let edge = |sub: u32| ((1023 - 37 * sub) as f32 + 0.5) / WAVE_SCALE;
    curve_at(&mut ram, 3, 0, [6, 0], [0.0, 1.0], &[1.0, 0.0, 0.0, 1.0]);
    let slid = rust_function(&mut lib, &ram, edge(6), 0.0).unwrap();
    assert_eq!(Ok(slid), guest_function(&ram, edge(6), 0.0));
    // Sub-type 5 does not wrap: the same two bytes interpolate to a half.
    curve_at(&mut ram, 3, 0, [5, 0], [0.0, 1.0], &[1.0, 0.0, 0.0, 1.0]);
    let plain = f32::from_bits(rust_function(&mut lib, &ram, edge(5), 0.0).unwrap());
    let wrapped = f32::from_bits(slid);
    assert!(
        wrapped > 0.9 || wrapped < 0.1,
        "the wrap moves the value to an end: {wrapped}"
    );
    assert!((plain - 0.5).abs() < 0.1, "unwrapped: {plain}");
    // A sub-type past the copied tables declines.
    curve_at(
        &mut ram,
        3,
        0,
        [WAVES as u8, 0],
        [0.0, 1.0],
        &[1.0, 0.0, 0.0, 1.0],
    );
    assert_eq!(rust_function(&mut lib, &ram, 0.5, 0.0), Err(Decline::Curve));
}

#[test]
fn classify_follows_the_compare_tree() {
    use Base::*;
    let cases = [
        (10, One),
        (11, Zero),
        (12, Flag),
        (20, Callout),
        (21, Variant),
        (22, Callout),
        (23, Callout),
        (30, Callout),
        (31, One),
        (32, Callout),
        (33, Callout),
        (40, Field(0xec)),
        (41, Callout),
        (42, Field(0xf0)),
        (50, Random),
        (51, Shield),
        (52, One),
        (53, Field(0xf8)),
        (60, Callout),
        (61, Field(0xf4)),
        (63, Callout),
        (64, Callout),
        (70, F0LessOne),
        (71, Callout),
        (72, World(0x70)),
        (73, World(0x74)),
        (80, World(0x78)),
        (81, World(0x7c)),
        (83, Callout),
        (84, Callout),
    ];
    for (name, want) in cases {
        assert_eq!(classify(name, &NAMES), want, "name {name}");
    }
    for name in [0u32, 5, 13, 24, 34, 43, 54, 62, 65, 74, 82, 85, u32::MAX] {
        assert_eq!(classify(name, &NAMES), Missing, "name {name}");
    }
}

fn value(lib: &mut Library, ram: &Ram, handle: u32, name: u32) -> Result<Answer, Decline> {
    lib.value(ram, ENV, handle, name)
}

/// An answer the base level gave: the value and its activity.
fn base(v: f32) -> Answer {
    let active = u8::from(v > 0.0);
    Answer {
        value: v.to_bits(),
        al: active,
        active,
        xmm0: Some(v.to_bits()),
    }
}

#[test]
fn the_built_in_names_and_a_null_handle() {
    let (ram, mut lib) = world();
    let one = Answer {
        value: 1.0f32.to_bits(),
        al: 1,
        active: 0,
        xmm0: Some(1.0f32.to_bits()),
    };
    assert_eq!(value(&mut lib, &ram, OBJECT, NAME_ONE), Ok(one));
    assert_eq!(value(&mut lib, &ram, OBJECT, 0), Ok(one));
    let zero = Answer {
        value: 0,
        al: 0,
        active: 0,
        xmm0: None,
    };
    assert_eq!(value(&mut lib, &ram, OBJECT, NAME_ZERO), Ok(zero));
    assert_eq!(value(&mut lib, &ram, NONE, E1), Ok(zero));
    // A name nothing knows, with no exports and no parent: 0.
    let none = Answer {
        value: 0,
        al: 0,
        active: 0,
        xmm0: Some(0),
    };
    assert_eq!(value(&mut lib, &ram, OBJECT, UNKNOWN), Ok(none));
}

#[test]
fn the_base_level_answers_its_names_and_declines_its_callouts() {
    let (mut ram, mut lib) = world();
    ram.setf(BODY + 0xec, 1.5);
    ram.setf(BODY + 0xf0, 0.5);
    ram.setf(BODY + 0xf4, 0.25);
    ram.setf(BODY + 0xf8, -2.0);
    ram.setf(WORLD + 0x70, 0.125);
    ram.setf(WORLD + 0x74, 0.375);
    ram.setf(WORLD + 0x78, 0.75);
    ram.setf(WORLD + 0x7c, 2.0);
    for (name, want) in [
        (10, 1.0),
        (31, 1.0),
        (52, 1.0),
        (11, 0.0),
        (40, 1.0),
        (42, 0.5),
        (61, 0.25),
        (53, 0.0),
        (70, 0.0),
        (72, 0.125),
        (73, 0.375),
        (80, 0.75),
        (81, 1.0),
        (51, 0.0),
    ] {
        assert_eq!(
            value(&mut lib, &ram, OBJECT, name),
            Ok(base(want)),
            "name {name}"
        );
    }
    for name in [20, 22, 23, 30, 32, 33, 41, 60, 63, 71, 83] {
        assert_eq!(
            value(&mut lib, &ram, OBJECT, name),
            Err(Decline::Import),
            "name {name}"
        );
    }
    // The flag: bit 2 of +10a clear is 1.
    assert_eq!(value(&mut lib, &ram, OBJECT, 12), Ok(base(1.0)));
    ram.set8(BODY + 0x10a, 4);
    assert_eq!(value(&mut lib, &ram, OBJECT, 12), Ok(base(0.0)));
    // The shield test: +f0 equal to zero is 1; in mode 2 a unit with a
    // +13c record asks 1588b0.
    ram.setf(BODY + 0xf0, -0.0);
    assert_eq!(value(&mut lib, &ram, OBJECT, 51), Ok(base(1.0)));
    ram.set32(GAME + 8, 2);
    assert_eq!(value(&mut lib, &ram, OBJECT, 51), Ok(base(1.0)));
    ram.set32(BODY + 0x13c, 7);
    assert_eq!(value(&mut lib, &ram, OBJECT, 51), Err(Decline::Import));
}

#[test]
fn the_variant_fraction_and_the_random_value() {
    let (mut ram, mut lib) = world();
    for (variant, want) in [(1i8, 0.5f32), (-2, 0.0), (10, 1.0), (0, 0.25)] {
        ram.set8(BODY + 0xb1, variant as u8);
        assert_eq!(
            value(&mut lib, &ram, OBJECT, 21),
            Ok(base(want)),
            "variant {variant}"
        );
    }
    ram.set32(MODEL + 0x50, 0);
    assert_eq!(
        value(&mut lib, &ram, OBJECT, 21),
        Ok(base(0.0)),
        "no variants"
    );
    let h = constants().random_hash.of(OBJECT);
    let want = h as f32 * RANDOM_SCALE;
    assert_eq!(value(&mut lib, &ram, OBJECT, 50), Ok(base(want.min(1.0))));
    let unready = Env {
        x87_ready: false,
        ..ENV
    };
    assert_eq!(lib.value(&ram, unready, OBJECT, 50), Err(Decline::Float));
}

#[test]
fn a_type_level_past_the_base_level_declines() {
    let (mut ram, mut lib) = world();
    ram.set8(BODY + 0xaa, 1);
    ram.setf(BODY + 0xf0, 0.5);
    // The base level is first: its names still answer.
    assert_eq!(value(&mut lib, &ram, OBJECT, 42), Ok(base(0.5)));
    // A name it does not know goes on to the unit level.
    assert_eq!(value(&mut lib, &ram, OBJECT, UNKNOWN), Err(Decline::Import));
}

/// Export `i` of the record table at `records`.
fn export(
    ram: &mut Ram,
    records: u32,
    i: u32,
    flags: u32,
    input: u32,
    name: u32,
    gate: u32,
    threshold: f32,
    scale: u32,
    data: u32,
) {
    let r = records + i * 0x20;
    ram.set32(r, flags);
    ram.set32(r + 4, input);
    ram.set32(r + 8, name);
    ram.set32(r + 0xc, gate);
    ram.setf(r + 0x10, threshold);
    ram.set32(r + 0x14, 1);
    ram.set32(r + 0x18, data);
    ram.set32(r + 0x1c, scale);
}

fn exports(ram: &mut Ram, def: u32, count: u32) {
    ram.set32(def + 0x64, count);
}

/// An export answer: value and al, which is also the frame byte.
fn found(v: f32, al: u8) -> Answer {
    Answer {
        value: v.to_bits(),
        al,
        active: al,
        xmm0: Some(v.to_bits()),
    }
}

#[test]
fn an_export_maps_its_input_through_its_function() {
    let (mut ram, mut lib) = world();
    let identity = blob(&mut ram, 0, 0, 0, [0, 0], [0.0, 1.0], &[]);
    ram.setf(BODY + 0xec, 0.25);
    ram.setf(BODY + 0xf8, 0.5);
    exports(&mut ram, DEF, 1);
    let case =
        |ram: &mut Ram, lib: &mut Library, flags: u32, gate: u32, threshold: f32, scale: u32| {
            export(
                ram, RECORDS, 0, flags, 40, E1, gate, threshold, scale, identity,
            );
            lib.definitions.clear();
            lib.value(&*ram, ENV, OBJECT, E1)
        };
    // Plain: the input, active above 0.
    assert_eq!(case(&mut ram, &mut lib, 0, 0, 0.0, 0), Ok(found(0.25, 1)));
    // Bit 0 inverts the input.
    assert_eq!(case(&mut ram, &mut lib, 1, 0, 0.0, 0), Ok(found(0.75, 1)));
    // A range keeps its input's activity: an inactive input is 0, unless
    // bit 2 forces it active.
    assert_eq!(case(&mut ram, &mut lib, 2, 0, 0.0, 0), Ok(found(0.25, 1)));
    ram.setf(BODY + 0xec, 0.0);
    assert_eq!(case(&mut ram, &mut lib, 2, 0, 0.0, 0), Ok(found(0.0, 0)));
    assert_eq!(case(&mut ram, &mut lib, 6, 0, 0.0, 0), Ok(found(0.0, 1)));
    ram.setf(BODY + 0xec, 0.25);
    // A positive threshold: active only above it.
    assert_eq!(case(&mut ram, &mut lib, 0, 0, 0.5, 0), Ok(found(0.0, 0)));
    assert_eq!(case(&mut ram, &mut lib, 0, 0, 0.2, 0), Ok(found(0.25, 1)));
    // The gate import: inactive (body +f4 is 0) zeroes it; active passes.
    assert_eq!(case(&mut ram, &mut lib, 0, 61, 0.0, 0), Ok(found(0.0, 0)));
    assert_eq!(case(&mut ram, &mut lib, 0, 53, 0.0, 0), Ok(found(0.25, 1)));
    // The scale: another value of the object multiplies it, and an
    // inactive one leaves it inactive.
    assert_eq!(case(&mut ram, &mut lib, 0, 0, 0.0, 53), Ok(found(0.125, 1)));
    assert_eq!(case(&mut ram, &mut lib, 0, 0, 0.0, 61), Ok(found(0.0, 0)));
    // An output range past 1 clamps at the end.
    let wide = blob(&mut ram, 1, 0, 0, [0, 0], [0.0, 8.0], &[]);
    export(&mut ram, RECORDS, 0, 0, 40, E1, 0, 0.0, 0, wide);
    lib.definitions.clear();
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(found(1.0, 1)));
}

#[test]
fn a_periodic_export_runs_on_game_time_with_an_optional_per_object_offset() {
    let (mut ram, mut lib) = world();
    let wave = blob(&mut ram, 0, 3, 0, [1, 0], [0.0, 1.0], &[0.5, 0.0, 0.0, 1.0]);
    exports(&mut ram, DEF, 1);
    let t = 1234.0f32 * (1.0f32 / 30.0);
    let h = constants().time_hash.of(OBJECT);
    let clamp = |v: f32| v.clamp(0.0, 1.0);
    let want = |ram: &mut Ram, t: f32| {
        reference(ram, wave);
        let v = f32::from_bits(guest_function(ram, t, 1.0).unwrap());
        found(clamp(v), 1)
    };
    // Input: the "one" import (1.0, active), which the curve scales.
    export(&mut ram, RECORDS, 0, 0, 10, E1, 0, 0.0, 0, wave);
    lib.definitions.clear();
    let plain = want(&mut ram, t);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(plain));
    // Bit 3 adds the handle's hash: single precision rounds each step,
    export(&mut ram, RECORDS, 0, 8, 10, E1, 0, 0.0, 0, wave);
    lib.definitions.clear();
    let single = Env {
        x87_control: 0x007f,
        ..ENV
    };
    let ts = h as f32 * TIME_SCALE + t;
    assert_eq!(lib.value(&ram, single, OBJECT, E1), Ok(want(&mut ram, ts)));
    // double rounds the sum to 53 bits and the store to 24,
    let td = (f64::from(h) * f64::from(TIME_SCALE) + f64::from(t)) as f32;
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(want(&mut ram, td)));
    // and extended is declined, as is an x87 not ready for the sequence.
    let extended = Env {
        x87_control: 0x037f,
        ..ENV
    };
    assert_eq!(lib.value(&ram, extended, OBJECT, E1), Err(Decline::Float));
    let unready = Env {
        x87_ready: false,
        ..ENV
    };
    assert_eq!(lib.value(&ram, unready, OBJECT, E1), Err(Decline::Float));
    // Without bit 3 the x87 is not used.
    export(&mut ram, RECORDS, 0, 0, 10, E1, 0, 0.0, 0, wave);
    lib.definitions.clear();
    assert_eq!(lib.value(&ram, extended, OBJECT, E1), Ok(plain));
}

#[test]
fn the_last_matching_export_wins_and_the_parent_answers_when_none_match() {
    let (mut ram, mut lib) = world();
    let identity = blob(&mut ram, 0, 0, 0, [0, 0], [0.0, 1.0], &[]);
    ram.setf(BODY + 0xec, 0.25);
    ram.setf(BODY + 0xf0, 0.5);
    exports(&mut ram, DEF, 3);
    export(&mut ram, RECORDS, 0, 0, 40, E1, 0, 0.0, 0, identity);
    export(&mut ram, RECORDS, 1, 0, 40, E3, 0, 0.0, 0, identity);
    export(&mut ram, RECORDS, 2, 0, 42, E1, 0, 0.0, 0, identity);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(found(0.5, 1)));
    // E2 is only the parent's; bit 26 of the flags asks it.
    ram.setf(PARENT_BODY + 0xec, 0.75);
    exports(&mut ram, PARENT_DEF, 1);
    export(&mut ram, PARENT_RECORDS, 0, 0, 40, E2, 0, 0.0, 0, identity);
    assert_eq!(
        value(&mut lib, &ram, OBJECT, E2),
        Ok(Answer {
            value: 0,
            al: 0,
            active: 0,
            xmm0: Some(0)
        })
    );
    // With the bit and a null parent, still 0.
    ram.set32(BODY + 4, 1 << 26);
    assert_eq!(
        value(&mut lib, &ram, OBJECT, E2),
        Ok(Answer {
            value: 0,
            al: 0,
            active: 0,
            xmm0: Some(0)
        })
    );
    ram.set32(BODY + 0x14, PARENT);
    assert_eq!(
        value(&mut lib, &ram, OBJECT, E2),
        Ok(Answer {
            active: 0,
            ..found(0.75, 1)
        })
    );
}

#[test]
fn an_override_a_loop_and_an_unpinned_environment_decline() {
    let (mut ram, mut lib) = world();
    let identity = blob(&mut ram, 0, 0, 0, [0, 0], [0.0, 1.0], &[]);
    ram.setf(BODY + 0xec, 0.25);
    exports(&mut ram, DEF, 1);
    // An export scaled by itself recurses until the depth runs out.
    export(&mut ram, RECORDS, 0, 0, 40, E1, 0, 0.0, E1, identity);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Err(Decline::Depth));
    export(&mut ram, RECORDS, 0, 0, 40, E1, 0, 0.0, 0, identity);
    lib.definitions.clear();
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(found(0.25, 1)));
    for (mxcsr, control) in [(0x1f80u32, 0x027fu16), (0x1fa0, 0x0e7f), (0x3fa0, 0x027f)] {
        let env = Env {
            mxcsr,
            x87_control: control,
            x87_ready: true,
        };
        assert_eq!(
            lib.value(&ram, env, OBJECT, E1),
            Err(Decline::Float),
            "{mxcsr:#x} {control:#x}"
        );
    }
    // A NaN input is off the exact path.
    ram.set32(BODY + 0xec, 0x7fc0_0000);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Err(Decline::Float));
    ram.set16(BODY + 0xc0, 0x200);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Err(Decline::Override));
    // Curve kinds 9 and 10 are the guest's.
    ram.set16(BODY + 0xc0, 0);
    ram.setf(BODY + 0xec, 0.25);
    let nine = blob(&mut ram, 1, 9, 0, [0, 0], [0.0, 1.0], &[]);
    export(&mut ram, RECORDS, 0, 0, 40, E1, 0, 0.0, 0, nine);
    lib.definitions.clear();
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Err(Decline::Curve));
}

#[test]
fn a_definition_is_decoded_once_and_again_when_it_changes() {
    let (mut ram, mut lib) = world();
    let identity = blob(&mut ram, 0, 0, 0, [0, 0], [0.0, 1.0], &[]);
    ram.setf(BODY + 0xec, 0.25);
    ram.setf(BODY + 0xf0, 0.5);
    exports(&mut ram, DEF, 1);
    export(&mut ram, RECORDS, 0, 0, 40, E1, 0, 0.0, 0, identity);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(found(0.25, 1)));
    // The copy is what answers: a changed record is not seen while the
    // count and table stay.
    export(&mut ram, RECORDS, 0, 0, 42, E1, 0, 0.0, 0, identity);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(found(0.25, 1)));
    assert_eq!(lib.decoded(), 1);
    // A new count is decoded again.
    exports(&mut ram, DEF, 2);
    export(&mut ram, RECORDS, 1, 0, 40, E3, 0, 0.0, 0, identity);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(found(0.5, 1)));
    assert_eq!(lib.decoded(), 1);
    // An export whose blob will not read declines only when it is used.
    export(&mut ram, RECORDS, 1, 0, 40, E3, 0, 0.0, 0, 0x00ff_0000);
    exports(&mut ram, DEF, 3);
    export(&mut ram, RECORDS, 2, 0, 40, E2, 0, 0.0, 0, identity);
    assert_eq!(value(&mut lib, &ram, OBJECT, E1), Ok(found(0.5, 1)));
    assert_eq!(value(&mut lib, &ram, OBJECT, E3), Err(Decline::Fault));
}

#[test]
fn blob_lengths_cover_what_the_curve_reads() {
    // The furthest param of each kind, blended, is inside the copy: the
    // sweep above would decline as a curve fault otherwise. Here, the
    // lengths themselves.
    assert_eq!(blob_len(0, false), 0x0c);
    assert_eq!(blob_len(2, true), 0x24);
    assert_eq!(blob_len(3, true), 0x34);
    assert_eq!(blob_len(4, true), 0x44);
    assert_eq!(blob_len(5, true), 0xb4);
    assert_eq!(blob_len(7, false), 0x44);
    assert_eq!(blob_len(7, true), 0x74);
    assert_eq!(
        Decline::ALL.map(Decline::name),
        ["float", "override", "import", "curve", "fault", "depth"]
    );
}
