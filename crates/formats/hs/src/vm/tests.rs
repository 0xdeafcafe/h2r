//! Synthetic scripts only: built node by node, no disc data.
use super::mock::{Policy, RecordingEngine};
use super::*;
use crate::{Builtin, Global, Script};

/// Retail builtin indices the native evaluators branch on.
mod op {
    pub const BEGIN: u16 = 0;
    pub const BEGIN_RANDOM: u16 = 1;
    pub const IF: u16 = 2;
    pub const COND: u16 = 3;
    pub const SET: u16 = 4;
    pub const AND: u16 = 5;
    pub const OR: u16 = 6;
    pub const ADD: u16 = 7;
    pub const SUB: u16 = 8;
    pub const MUL: u16 = 9;
    pub const DIV: u16 = 10;
    pub const MIN: u16 = 11;
    pub const MAX: u16 = 12;
    pub const EQ: u16 = 13;
    pub const NE: u16 = 14;
    pub const GT: u16 = 15;
    pub const LT: u16 = 16;
    pub const GE: u16 = 17;
    pub const LE: u16 = 18;
    pub const SLEEP: u16 = 19;
    pub const SLEEP_FOREVER: u16 = 20;
    pub const SLEEP_UNTIL: u16 = 21;
    pub const WAKE: u16 = 22;
    pub const UNIT: u16 = 24;
    pub const NOT: u16 = 25;
    pub const PIN: u16 = 26;
    pub const PRINT: u16 = 27;
    // Synthetic engine-facing builtins.
    pub const LOG: u16 = 28; // (short) -> void
    pub const TEST: u16 = 29; // () -> boolean
    pub const PROBE: u16 = 30; // (real) -> real
    pub const RANDOM_RANGE: u16 = 31;
    pub const REAL_RANDOM_RANGE: u16 = 32;
}
const VOID: u16 = 4;
const BOOL: u16 = 5;
const REAL: u16 = 6;
const SHORT: u16 = 7;
const LONG: u16 = 8;
const STRING: u16 = 9;
const SCRIPT: u16 = 10;
const PASS: u16 = 3;

fn catalog() -> Catalog {
    let mut functions = Vec::new();
    let mut push = |evaluate: u32, ret: u16, flags: u16, params: &[u16], name: &str| {
        functions.push(Builtin {
            descriptor: 0,
            return_type: ret,
            flags,
            evaluate,
            usage: None,
            parameters: params.to_vec(),
            name: Some(name.into()),
        })
    };
    let n = |i: usize| Native::BY_ADDRESS[i].0;
    push(n(0), PASS, 2, &[], "begin");
    push(n(1), PASS, 2, &[], "begin_random");
    push(n(2), PASS, 2, &[], "if");
    push(0, PASS, 2, &[], "cond");
    push(n(3), PASS, 2, &[], "set");
    push(n(4), BOOL, 2, &[], "and");
    push(n(4), BOOL, 2, &[], "or");
    for name in ["+", "-", "*", "/", "min", "max"] {
        push(n(5), REAL, 2, &[], name);
    }
    push(n(6), BOOL, 2, &[], "=");
    push(n(6), BOOL, 2, &[], "!=");
    for name in [">", "<", ">=", "<="] {
        push(n(7), BOOL, 2, &[], name);
    }
    push(n(8), VOID, 2, &[], "sleep");
    push(n(9), VOID, 2, &[], "sleep_forever");
    push(n(10), VOID, 2, &[], "sleep_until");
    push(n(11), VOID, 2, &[], "wake");
    push(n(12), VOID, 2, &[], "inspect");
    push(n(13), 51, 2, &[], "unit");
    push(n(14), BOOL, 0, &[BOOL], "not");
    push(n(15), REAL, 0, &[REAL, REAL, REAL], "pin");
    push(0x0030_0000, VOID, 0, &[STRING], "print");
    push(0x0030_0010, VOID, 0, &[SHORT], "log");
    push(0x0030_0020, BOOL, 0, &[], "test");
    push(0x0030_0030, REAL, 0, &[REAL], "probe");
    push(n(16), SHORT, 0, &[SHORT, SHORT], "random_range");
    push(n(17), REAL, 0, &[REAL, REAL], "real_random_range");
    Catalog {
        functions,
        types: (0..62).map(|i| format!("type{i}")).collect(),
        script_kinds: [
            "startup",
            "dormant",
            "continuous",
            "static",
            "stub",
            "command_script",
        ]
        .map(str::to_owned)
        .to_vec(),
        external_global_types: vec![BOOL, REAL],
    }
}
fn image() -> VmImage {
    let mut image = VmImage::without_stubs();
    image
        .discarded
        .insert(op::PRINT, Native::DiscardedEvaluated);
    image
}

#[derive(Default)]
struct B {
    nodes: Vec<Node>,
    strings: Vec<u8>,
    scripts: Vec<Script>,
    globals: Vec<Global>,
}
fn datum(slot: usize) -> Datum {
    Datum(((0x8000 + slot as u32) << 16) | slot as u32)
}
fn fixed(s: &str) -> [u8; 32] {
    let mut b = [0; 32];
    b[..s.len()].copy_from_slice(s.as_bytes());
    b
}
impl B {
    fn push(
        &mut self,
        opcode: u16,
        value_type: u16,
        flags: u16,
        string_offset: u32,
        data: u32,
    ) -> Datum {
        let slot = self.nodes.len();
        self.nodes.push(Node {
            salt: datum(slot).salt(),
            opcode,
            value_type,
            flags,
            next: Datum::NULL,
            string_offset,
            data,
        });
        datum(slot)
    }
    /// Literal of source type `src`, read as `ty`.
    fn lit(&mut self, src: u16, ty: u16, data: u32) -> Datum {
        self.push(src, ty, 9, u32::MAX, data)
    }
    fn real(&mut self, r: f32) -> Datum {
        self.lit(REAL, REAL, r.to_bits())
    }
    fn short(&mut self, s: i16) -> Datum {
        self.lit(SHORT, SHORT, s as u16 as u32)
    }
    fn boolean(&mut self, b: bool) -> Datum {
        self.lit(BOOL, BOOL, b as u32)
    }
    fn string(&mut self, s: &str) -> Datum {
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(s.as_bytes());
        self.strings.push(0);
        self.push(STRING, STRING, 9, offset, 0)
    }
    fn script_ref(&mut self, index: u16) -> Datum {
        self.lit(SCRIPT, SCRIPT, u32::from(index))
    }
    fn global(&mut self, index: u16, ty: u16) -> Datum {
        let offset = self.strings.len() as u32;
        let name = self.globals[index as usize].name().unwrap().to_owned();
        self.strings.extend_from_slice(name.as_bytes());
        self.strings.push(0);
        self.push(0, ty, 13, offset, u32::from(index))
    }
    fn external(&mut self, index: u16, ty: u16) -> Datum {
        self.push(0, ty, 13, u32::MAX, u32::from(index | 0x8000))
    }
    fn head_and_args(&mut self, op: u16, args: &[Datum]) -> Datum {
        let head = self.push(op, 2, 9, u32::MAX, 0);
        let mut prev = head;
        for &a in args {
            self.nodes[prev.slot()].next = a;
            prev = a;
        }
        head
    }
    fn call(&mut self, op: u16, ty: u16, args: &[Datum]) -> Datum {
        let head = self.head_and_args(op, args);
        self.push(op, ty, 8, u32::MAX, head.0)
    }
    fn call_script(&mut self, index: u16, ty: u16) -> Datum {
        let head = self.head_and_args(index, &[]);
        self.push(index, ty, 10, u32::MAX, head.0)
    }
    fn script(&mut self, name: &str, kind: u16, ret: u16, root: Datum) -> u16 {
        self.scripts.push(Script {
            name_bytes: fixed(name),
            kind,
            return_type: ret,
            root,
        });
        self.scripts.len() as u16 - 1
    }
    fn define_global(&mut self, name: &str, ty: u16, root: Datum) -> u16 {
        self.globals.push(Global {
            name_bytes: fixed(name),
            value_type: ty,
            reserved: 0,
            root,
        });
        self.globals.len() as u16 - 1
    }
    fn build(self) -> Scenario {
        let s = Scenario {
            nodes: self.nodes,
            strings: self.strings,
            scripts: self.scripts,
            globals: self.globals,
            references: vec![],
        };
        s.validate_graph().unwrap();
        s
    }
}

struct Run {
    engine: RecordingEngine,
}
impl Run {
    fn new(policy: Policy) -> Self {
        let mut engine = RecordingEngine::new(policy);
        engine.keep = 1000;
        Self { engine }
    }
    /// Initialize and tick `0..ticks`.
    fn go(&mut self, vm: &mut Vm<'_>, ticks: i32) {
        vm.initialize(&mut self.engine).unwrap();
        self.tick(vm, 0, ticks);
    }
    fn tick(&mut self, vm: &mut Vm<'_>, from: i32, to: i32) {
        for t in from..to {
            self.engine.time = t;
            vm.update(&mut self.engine);
        }
    }
    /// (tick, first argument) of every `log` call.
    fn logs(&self) -> Vec<(i32, i16)> {
        self.engine
            .log
            .iter()
            .filter(|c| c.index == op::LOG)
            .map(|c| (c.tick, c.arguments[0] as i16))
            .collect()
    }
}
fn log(b: &mut B, n: i16) -> Datum {
    let a = b.short(n);
    b.call(op::LOG, VOID, &[a])
}

#[test]
fn begin_set_and_arithmetic_fold_left_in_single_precision() {
    let mut b = B::default();
    let zero = b.real(0.0);
    let g = b.define_global("g", REAL, zero);
    let (one, two, three, four) = (b.real(1.0), b.real(2.0), b.real(3.0), b.real(4.0));
    let mul = b.call(op::MUL, REAL, &[two, three]);
    let sum = b.call(op::ADD, REAL, &[one, mul, four]);
    let target = b.global(g, REAL);
    let set = b.call(op::SET, REAL, &[target, sum]);
    let (a, c) = (b.real(10.0), b.real(4.0));
    let sub = b.call(op::SUB, REAL, &[a, c]);
    let div_by = b.real(0.0);
    let div = b.call(op::DIV, REAL, &[sub, div_by]);
    let target2 = b.global(g, REAL);
    let set2 = b.call(op::SET, REAL, &[target2, div]);
    let s = b.short(0);
    let sleep = b.call(op::SLEEP, VOID, &[s]);
    let root = b.call(op::BEGIN, VOID, &[set, sleep]);
    b.script("main", 0, VOID, root);
    let root2 = b.call(op::BEGIN, VOID, &[set2]);
    b.script("later", 1, VOID, root2);
    let (s, c) = (b.build(), catalog());
    let image = image();
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 1);
    assert_eq!(vm.global_by_name("g"), Some(Value::Real(11.0)));
    assert_eq!(vm.thread(0).unwrap().state(), ThreadState::Finished);
    vm.wake_script(1).unwrap();
    run.tick(&mut vm, 1, 2);
    assert_eq!(vm.global_by_name("g"), Some(Value::Real(f32::INFINITY)));
    assert!(vm.faults().is_empty());
}

#[test]
fn min_max_keep_the_retail_unordered_choice() {
    let mut b = B::default();
    let eval = |b: &mut B, opc: u16, x: f32, y: f32| {
        let (x, y) = (b.real(x), b.real(y));
        b.call(opc, REAL, &[x, y])
    };
    let cases = [
        eval(&mut b, op::MIN, 3.0, 2.0),
        eval(&mut b, op::MAX, 3.0, 2.0),
        eval(&mut b, op::MIN, 1.0, f32::NAN),
        eval(&mut b, op::MAX, 1.0, f32::NAN),
    ];
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut e = RecordingEngine::new(Policy::Neutral);
    vm.initialize(&mut e).unwrap();
    let got: Vec<f32> = cases
        .iter()
        .map(|&d| vm.evaluate(&mut e, d, false).unwrap().unwrap().real())
        .collect();
    assert_eq!(&got[..2], &[2.0, 3.0]);
    assert_eq!(got[2], 1.0);
    assert!(got[3].is_nan());
}

#[test]
fn if_without_else_returns_zero_and_cond_is_a_null_evaluator() {
    let mut b = B::default();
    let f = b.boolean(false);
    let then = b.short(7);
    let iff = b.call(op::IF, SHORT, &[f, then]);
    let t = b.boolean(true);
    let (x, y) = (b.short(1), b.short(2));
    let iff2 = b.call(op::IF, SHORT, &[t, x, y]);
    let cond = b.call(op::COND, VOID, &[]);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut e = RecordingEngine::new(Policy::Neutral);
    vm.initialize(&mut e).unwrap();
    assert_eq!(vm.evaluate(&mut e, iff, false).unwrap(), Ok(Cell::ZERO));
    assert_eq!(
        vm.evaluate(&mut e, iff2, false).unwrap(),
        Ok(Cell::from_short(1))
    );
    assert_eq!(
        vm.evaluate(&mut e, cond, false),
        Err(VmError::NullEvaluator { index: op::COND })
    );
}

#[test]
fn and_or_short_circuit_and_fold_bytes() {
    let mut b = B::default();
    let t = b.boolean(true);
    let probe = b.call(op::TEST, BOOL, &[]);
    let or = b.call(op::OR, BOOL, &[t, probe]);
    let f = b.boolean(false);
    let probe2 = b.call(op::TEST, BOOL, &[]);
    let and = b.call(op::AND, BOOL, &[f, probe2]);
    let t2 = b.boolean(true);
    let probe3 = b.call(op::TEST, BOOL, &[]);
    let and2 = b.call(op::AND, BOOL, &[t2, probe3]);
    let empty_and = b.call(op::AND, BOOL, &[]);
    let empty_or = b.call(op::OR, BOOL, &[]);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut e = RecordingEngine::new(Policy::Truthy);
    vm.initialize(&mut e).unwrap();
    let mut v = |d| vm.evaluate(&mut e, d, false).unwrap().unwrap().boolean();
    assert!(v(or));
    assert!(!v(and));
    assert!(v(and2));
    assert!(v(empty_and));
    assert!(!v(empty_or));
    assert_eq!(e.counts.get(&op::TEST), Some(&1));
}

#[test]
fn casts_follow_the_retail_table_including_its_inverted_booleans() {
    let mut b = B::default();
    let cases = [
        // boolean from real is `bits == 0`.
        (b.lit(REAL, BOOL, 0.0f32.to_bits()), Cell(1)),
        (
            b.lit(REAL, BOOL, 2.5f32.to_bits()),
            Cell(2.5f32.to_bits() & !0xff),
        ),
        (b.lit(SHORT, BOOL, 0), Cell(1)),
        // short from real truncates toward zero; NaN is the indefinite.
        (
            b.lit(REAL, SHORT, 2.7f32.to_bits()),
            Cell((2.7f32.to_bits() & !0xffff) | 2),
        ),
        (
            b.lit(REAL, SHORT, (-2.7f32).to_bits()),
            Cell(((-2.7f32).to_bits() & !0xffff) | 0xfffe),
        ),
        (b.lit(REAL, LONG, f32::NAN.to_bits()), Cell(0x8000_0000)),
        (b.lit(SHORT, LONG, 0xffff), Cell(u32::MAX)),
        (b.lit(LONG, SHORT, 0x0001_0005), Cell(0x0001_0005)),
        (b.lit(SHORT, REAL, 0xfffd), Cell::from_real(-3.0)),
        // Enumerations convert to real as value + 1.
        (b.lit(44, REAL, 2), Cell::from_real(3.0)),
        (b.lit(REAL, VOID, 1.0f32.to_bits()), Cell::ZERO),
        // unit -> object passes the handle.
        (b.lit(51, 50, 0x1234_5678), Cell(0x1234_5678)),
    ];
    let bad = b.lit(STRING, REAL, 0);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut e = RecordingEngine::new(Policy::Neutral);
    vm.initialize(&mut e).unwrap();
    for (d, want) in cases {
        assert_eq!(vm.evaluate(&mut e, d, false).unwrap(), Ok(want), "{d:?}");
    }
    assert_eq!(
        vm.evaluate(&mut e, bad, false),
        Err(VmError::InvalidCast {
            from: HsType::STRING,
            to: HsType::REAL
        })
    );
    assert_eq!(classify(HsType(57), HsType::UNIT), Cast::ObjectFromName);
    assert_eq!(
        classify(HsType::AI, HsType::OBJECT_LIST),
        Cast::ObjectListFromAi
    );
    assert_eq!(classify(HsType::PASSTHROUGH, HsType::REAL), Cast::Identity);
}

#[test]
fn comparisons_use_the_first_operand_type_and_single_precision() {
    let mut b = B::default();
    let big = |b: &mut B, v: i32| b.lit(LONG, LONG, v as u32);
    let (x, y) = (big(&mut b, 16_777_217), big(&mut b, 16_777_216));
    let gt = b.call(op::GT, BOOL, &[x, y]);
    let (x, y) = (big(&mut b, 16_777_217), big(&mut b, 16_777_216));
    let ge = b.call(op::GE, BOOL, &[x, y]);
    let (x, y) = (big(&mut b, 16_777_217), big(&mut b, 16_777_216));
    let eq = b.call(op::EQ, BOOL, &[x, y]);
    let (x, y) = (b.short(3), b.short(5));
    let lt = b.call(op::LT, BOOL, &[x, y]);
    let (x, y) = (b.short(3), b.short(3));
    let le = b.call(op::LE, BOOL, &[x, y]);
    let (x, y) = (b.string("a"), b.string("a"));
    let ne_strings = b.call(op::NE, BOOL, &[x, y]);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut e = RecordingEngine::new(Policy::Neutral);
    vm.initialize(&mut e).unwrap();
    let mut v = |d| vm.evaluate(&mut e, d, false).unwrap().unwrap().boolean();
    assert!(!v(gt), "longs compare after rounding to single");
    assert!(v(ge));
    assert!(!v(eq), "= compares the four bytes");
    assert!(v(lt));
    assert!(v(le));
    assert!(v(ne_strings), "distinct literals are distinct pointers");
}

#[test]
fn sleep_scales_by_tick_rate_and_zero_does_not_yield() {
    let mut b = B::default();
    let (l1, l2, l3) = (log(&mut b, 1), log(&mut b, 2), log(&mut b, 3));
    let (z, n) = (b.short(0), b.short(30));
    let (s0, s30) = (b.call(op::SLEEP, VOID, &[z]), b.call(op::SLEEP, VOID, &[n]));
    let root = b.call(op::BEGIN, VOID, &[l1, s0, l2, s30, l3]);
    b.script("main", 0, VOID, root);
    let s = b.build();
    let (c, image) = (catalog(), image());
    for (rate, wake) in [(30, 30), (60, 60)] {
        let mut vm = Vm::new(&s, &c, &image);
        let mut run = Run::new(Policy::Neutral);
        run.engine.tick_rate = rate;
        run.go(&mut vm, 200);
        assert_eq!(run.logs(), [(0, 1), (0, 2), (wake, 3)]);
    }
}

#[test]
fn sleep_until_always_waits_one_period_then_polls_and_times_out() {
    let mut b = B::default();
    let t = b.call(op::TEST, BOOL, &[]);
    let wait = b.call(op::SLEEP_UNTIL, VOID, &[t]);
    let l = log(&mut b, 1);
    let root = b.call(op::BEGIN, VOID, &[wait, l]);
    b.script("immediate", 0, VOID, root);

    let f = b.boolean(false);
    let (p, to) = (b.short(5), b.lit(LONG, LONG, 12));
    let wait2 = b.call(op::SLEEP_UNTIL, VOID, &[f, p, to]);
    let l2 = log(&mut b, 2);
    let root2 = b.call(op::BEGIN, VOID, &[wait2, l2]);
    b.script("timeout", 0, VOID, root2);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Truthy);
    run.go(&mut vm, 100);
    // True from the start, but first tested one default period (30) later.
    // The false wait polls at 5 and 10, then wakes at the 12-tick timeout.
    assert_eq!(run.logs(), [(12, 2), (30, 1)]);
    assert_eq!(run.engine.counts[&op::TEST], 1);
}

#[test]
fn thread_kinds_dormant_wake_continuous_and_static() {
    let mut b = B::default();
    let l1 = log(&mut b, 1);
    b.script("start", 0, VOID, l1);
    let l2 = log(&mut b, 2);
    let dormant = b.script("dormant", 1, VOID, l2);
    let l3 = log(&mut b, 3);
    let n = b.short(10);
    let s = b.call(op::SLEEP, VOID, &[n]);
    let root = b.call(op::BEGIN, VOID, &[l3, s]);
    b.script("loop", 2, VOID, root);
    let l4 = log(&mut b, 4);
    b.script("helper", 3, VOID, l4);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 25);
    assert_eq!(vm.threads().count(), 3, "static scripts get no thread");
    // A continuous thread that wakes from `(sleep 10)` at tick 10 completes
    // there with wake 0 and restarts on the *next* update: period 11.
    assert_eq!(run.logs(), [(0, 1), (0, 3), (11, 3), (22, 3)]);
    assert_eq!(vm.thread(0).unwrap().state(), ThreadState::Finished);
    assert_eq!(vm.thread(1).unwrap().state(), ThreadState::Dormant);
    vm.wake_script(dormant).unwrap();
    run.tick(&mut vm, 25, 26);
    assert_eq!(run.logs().last(), Some(&(25, 2)));
    assert_eq!(vm.thread(1).unwrap().state(), ThreadState::Finished);
    // Waking a finished thread does nothing.
    vm.wake_script(dormant).unwrap();
    run.tick(&mut vm, 26, 27);
    assert_eq!(run.logs().iter().filter(|l| l.1 == 2).count(), 1);
}

#[test]
fn wake_from_an_earlier_slot_runs_the_target_in_the_same_tick() {
    let mut b = B::default();
    let l = log(&mut b, 9);
    // Slot 0 wakes slot 1 (a later slot): it runs this tick.
    let r = b.script_ref(1);
    let w = b.call(op::WAKE, VOID, &[r]);
    b.script("waker", 0, VOID, w);
    b.script("sleeper", 1, VOID, l);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 1);
    assert_eq!(run.logs(), [(0, 9)]);
}

#[test]
fn wake_abandons_sleep_until() {
    let mut b = B::default();
    let f = b.boolean(false);
    let wait = b.call(op::SLEEP_UNTIL, VOID, &[f]);
    let l = log(&mut b, 1);
    let root = b.call(op::BEGIN, VOID, &[wait, l]);
    let waiter = b.script("waiter", 0, VOID, root);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 40);
    assert!(run.logs().is_empty());
    vm.wake_script(waiter).unwrap();
    run.tick(&mut vm, 40, 41);
    assert_eq!(run.logs(), [(40, 1)]);
}

#[test]
fn sleeping_another_thread_saves_its_state_for_wake() {
    let mut b = B::default();
    let l = log(&mut b, 5);
    let (n, r) = (b.short(100), b.script_ref(1));
    let sleep_other = b.call(op::SLEEP, VOID, &[n, r]);
    let r2 = b.script_ref(1);
    let wake = b.call(op::WAKE, VOID, &[r2]);
    let root = b.call(op::BEGIN, VOID, &[sleep_other, wake]);
    b.script("driver", 0, VOID, root);
    b.script("dormant", 1, VOID, l);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 200);
    // `wake` restores the saved -2 instead of starting it.
    assert!(run.logs().is_empty());
    assert_eq!(vm.thread(1).unwrap().state(), ThreadState::Dormant);
}

#[test]
fn sleep_forever_then_wake_resumes_after_the_call() {
    let mut b = B::default();
    let (l1, l2) = (log(&mut b, 1), log(&mut b, 2));
    let sf = b.call(op::SLEEP_FOREVER, VOID, &[]);
    let root = b.call(op::BEGIN, VOID, &[l1, sf, l2]);
    let me = b.script("main", 0, VOID, root);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 10);
    assert_eq!(vm.thread(0).unwrap().state(), ThreadState::Dormant);
    vm.wake_script(me).unwrap();
    run.tick(&mut vm, 10, 11);
    assert_eq!(run.logs(), [(0, 1), (10, 2)]);
}

#[test]
fn globals_initialize_in_order_and_script_calls_cast_their_result() {
    let mut b = B::default();
    let three = b.short(3);
    let a = b.define_global("a", SHORT, three);
    let ga = b.global(a, SHORT);
    let one = b.short(1);
    let sum = b.call(op::RANDOM_RANGE, SHORT, &[ga, one]);
    b.define_global("b", SHORT, sum);
    // A static script returning short, called where a real is expected.
    let seven = b.short(7);
    let helper = b.script("helper", 3, SHORT, seven);
    let call = b.call_script(helper, REAL);
    let g = b.real(0.0);
    let r = b.define_global("r", REAL, g);
    let target = b.global(r, REAL);
    let set = b.call(op::SET, REAL, &[target, call]);
    b.script("main", 0, VOID, set);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.engine.seed = 0x1234_5678;
    run.go(&mut vm, 1);
    assert_eq!(vm.global_by_name("a"), Some(Value::Short(3)));
    // random_range 3 1: seed' = 0x1234_5678 * 0x19660d + 0x3c6ef35f.
    let seed = 0x1234_5678u32
        .wrapping_mul(0x19660d)
        .wrapping_add(0x3c6ef35f);
    let want = 3u16.wrapping_add(((-2i32 as u32).wrapping_mul(seed >> 16) >> 16) as u16) as i16;
    assert_eq!(vm.global_by_name("b"), Some(Value::Short(want)));
    assert_eq!(vm.global_by_name("r"), Some(Value::Real(7.0)));
    assert_eq!(vm.stats().casts.get(&(SHORT, REAL)), Some(&1));
}

#[test]
fn begin_random_runs_every_branch_once() {
    let mut b = B::default();
    let logs: Vec<_> = (0..5).map(|i| log(&mut b, i)).collect();
    let root = b.call(op::BEGIN_RANDOM, VOID, &logs);
    b.script("main", 0, VOID, root);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.engine.seed = 42;
    run.go(&mut vm, 1);
    // Order and final seed computed by hand from the LCG
    // (seed * 0x19660d + 0x3c6ef35f) and the probe
    // (i + ((seed >> 16) * 5 >> 16)) % 5: five picks, then the all-used entry.
    let got: Vec<_> = run.logs().iter().map(|l| l.1).collect();
    assert_eq!(got, [1, 0, 2, 3, 4]);
    assert_eq!(run.engine.seed, 0x0691_e8e0);
}

#[test]
fn pin_not_print_and_overrides() {
    let mut b = B::default();
    let (x, lo, hi) = (b.real(5.0), b.real(0.0), b.real(2.0));
    let pin = b.call(op::PIN, REAL, &[x, lo, hi]);
    let t = b.boolean(true);
    let not = b.call(op::NOT, BOOL, &[t]);
    let text = b.string("hello");
    let print = b.call(op::PRINT, VOID, &[text]);
    let arg = b.real(2.0);
    let probe = b.call(op::PROBE, REAL, &[arg]);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    vm.bindings.bind(
        op::PROBE,
        Box::new(|_, call| Ok(Cell::from_real(call.arguments[0].real() * 10.0))),
    );
    assert_eq!(vm.binding(op::PROBE), Binding::Override);
    assert_eq!(
        vm.binding(op::PRINT),
        Binding::Native(Native::DiscardedEvaluated)
    );
    let mut e = RecordingEngine::new(Policy::Neutral);
    vm.initialize(&mut e).unwrap();
    let mut v = |d| vm.evaluate(&mut e, d, false).unwrap().unwrap();
    assert_eq!(v(pin), Cell::from_real(2.0));
    assert_eq!(v(not), Cell::from_bool(false));
    assert_eq!(v(print), Cell::ZERO);
    assert_eq!(v(probe), Cell::from_real(20.0));
    assert_eq!(e.prints, [b"hello".to_vec()]);
    assert!(e.counts.is_empty(), "nothing reached the engine");
}

#[test]
fn external_globals_refresh_and_write_back() {
    let mut b = B::default();
    let ext = b.external(1, REAL);
    let v = b.real(4.0);
    let set = b.call(op::SET, REAL, &[ext, v]);
    let read = b.external(0, BOOL);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut e = RecordingEngine::new(Policy::Neutral);
    vm.initialize(&mut e).unwrap();
    // Unbound externals read as the null-pointer default.
    assert_eq!(vm.evaluate(&mut e, set, false).unwrap(), Ok(Cell::ZERO));
    assert_eq!(e.external_writes.get(&1), Some(&1));
    assert_eq!(vm.evaluate(&mut e, read, false).unwrap(), Ok(Cell::ZERO));
}

#[test]
fn unbounded_recursion_overflows_the_thread_stack() {
    let mut b = B::default();
    let call = b.call_script(0, VOID);
    let root = b.call(op::BEGIN, VOID, &[call]);
    b.script("again", 0, VOID, root);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 1);
    assert_eq!(vm.faults().len(), 1);
    assert_eq!(vm.faults()[0].error, VmError::StackOverflow);
    assert_eq!(vm.thread(0).unwrap().state(), ThreadState::Finished);
}

#[test]
fn unit_cast_tests_the_object_type_mask() {
    let mut b = B::default();
    let o = b.lit(50, 50, 0x0001_0002);
    let unit = b.call(op::UNIT, 51, &[o]);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    for (policy, want) in [
        (Policy::Truthy, Cell(0x0001_0002)),
        (Policy::Neutral, Cell::NONE),
    ] {
        let mut e = RecordingEngine::new(policy);
        vm.initialize(&mut e).unwrap();
        assert_eq!(vm.evaluate(&mut e, unit, false).unwrap(), Ok(want));
    }
}

#[test]
fn scheduler_tick_order_is_slot_order_with_one_tick_read() {
    let mut b = B::default();
    let (l1, l2) = (log(&mut b, 1), log(&mut b, 2));
    let (n1, n2) = (b.short(1), b.short(1));
    let (s1, s2) = (
        b.call(op::SLEEP, VOID, &[n1]),
        b.call(op::SLEEP, VOID, &[n2]),
    );
    let r1 = b.call(op::BEGIN, VOID, &[l1, s1]);
    let r2 = b.call(op::BEGIN, VOID, &[l2, s2]);
    b.script("first", 2, VOID, r1);
    b.script("second", 2, VOID, r2);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 3);
    // Both resume at tick 1, complete, and restart at tick 2, in slot order.
    assert_eq!(run.logs(), [(0, 1), (0, 2), (2, 1), (2, 2)]);
}

#[test]
fn sleep_forever_evaluates_a_global_script_argument() {
    let mut b = B::default();
    let one = b.script_ref(1);
    let g = b.define_global("target", SCRIPT, one);
    let target = b.global(g, SCRIPT);
    let sf = b.call(op::SLEEP_FOREVER, VOID, &[target]);
    let l = log(&mut b, 10);
    let root = b.call(op::BEGIN, VOID, &[sf, l]);
    b.script("caller", 0, VOID, root);
    let l2 = log(&mut b, 20);
    b.script("victim", 0, VOID, l2);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 1);
    // Global 0 holds script 1: that thread sleeps, not the caller.
    assert_eq!(run.logs(), [(0, 10)]);
    assert_eq!(vm.thread(1).unwrap().state(), ThreadState::Dormant);
}

#[test]
fn equality_compares_only_the_type_width() {
    let mut b = B::default();
    // Same low byte, different upper bytes: booleans compare one byte.
    let (x, y) = (
        b.lit(BOOL, BOOL, 0x0000_0101),
        b.lit(BOOL, BOOL, 0x0000_7701),
    );
    let bools = b.call(op::EQ, BOOL, &[x, y]);
    // Shorts compare two bytes.
    let (x, y) = (
        b.lit(SHORT, SHORT, 0x0001_0005),
        b.lit(SHORT, SHORT, 0x0002_0005),
    );
    let shorts = b.call(op::EQ, BOOL, &[x, y]);
    let (x, y) = (
        b.lit(SHORT, SHORT, 0x0000_0105),
        b.lit(SHORT, SHORT, 0x0000_0005),
    );
    let shorts_differ = b.call(op::NE, BOOL, &[x, y]);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut e = RecordingEngine::new(Policy::Neutral);
    vm.initialize(&mut e).unwrap();
    let mut v = |d| vm.evaluate(&mut e, d, false).unwrap().unwrap().boolean();
    assert!(v(bools));
    assert!(v(shorts));
    assert!(v(shorts_differ));
}

/// The seed that makes the next LCG output `next` (inverse of
/// `seed * 0x19660d + 0x3c6ef35f` mod 2^32).
fn seed_before(next: u32) -> u32 {
    // Newton iteration for the inverse of an odd multiplier mod 2^32.
    let a: u32 = 0x0019_660d;
    let mut inv = a;
    for _ in 0..5 {
        inv = inv.wrapping_mul(2u32.wrapping_sub(a.wrapping_mul(inv)));
    }
    next.wrapping_sub(0x3c6e_f35f).wrapping_mul(inv)
}

#[test]
fn real_random_range_scales_by_one_over_65535() {
    let mut b = B::default();
    let (lo, hi) = (b.real(0.0), b.real(10.0));
    let rr = b.call(op::REAL_RANDOM_RANGE, REAL, &[lo, hi]);
    let s = b.build();
    let (c, image) = (catalog(), image());
    let mut vm = Vm::new(&s, &c, &image);
    let mut e = RecordingEngine::new(Policy::Neutral);
    vm.initialize(&mut e).unwrap();
    // seed >> 16 == 65535 reaches the upper endpoint exactly.
    e.seed = seed_before(0xffff_1234);
    assert_eq!(
        vm.evaluate(&mut e, rr, false).unwrap().unwrap().real(),
        10.0
    );
    assert_eq!(e.seed, 0xffff_1234);
    // seed >> 16 == 32768: 10 * 32768 / 65535, a little above 5.
    e.seed = seed_before(0x8000_1234);
    let mid = vm.evaluate(&mut e, rr, false).unwrap().unwrap().real();
    assert!((mid - 5.000_076_3).abs() < 2e-6, "{mid}");
    assert_ne!(mid, 5.0);
}

#[test]
fn stub_shapes_require_both_bodies_and_their_call_targets() {
    use super::image::{ARGUMENTS, RETURN, stub_shape_bytes};
    use iced_x86::code_asm::*;
    const A: u32 = 0x0030_0000;
    fn assemble(body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
        let mut a = CodeAssembler::new(32).unwrap();
        body(&mut a).unwrap();
        a.assemble(u64::from(A)).unwrap()
    }
    let unevaluated = |target: u32, slot: i32| {
        assemble(|a| {
            a.mov(ecx, dword_ptr(esp + slot))?;
            a.xor(eax, eax)?;
            a.call(u64::from(target))?;
            a.ret_1(12)
        })
    };
    let evaluated = |table: u32, arguments: u32, ret: u32, cleanup: i32| {
        assemble(|a| {
            let mut done = a.create_label();
            a.movsx(eax, word_ptr(esp + 4))?;
            a.mov(eax, dword_ptr(eax * 4 + table as i32))?;
            a.mov(ecx, dword_ptr(esp + 12))?;
            a.push(esi)?;
            a.mov(esi, dword_ptr(esp + 12))?;
            a.push(ecx)?;
            a.lea(edx, ptr(eax + 14))?;
            a.movsx(eax, word_ptr(eax + 12))?;
            a.push(edx)?;
            a.push(eax)?;
            a.push(esi)?;
            a.call(u64::from(arguments))?;
            a.test(eax, eax)?;
            a.je(done)?;
            a.xor(eax, eax)?;
            a.mov(ecx, esi)?;
            a.call(u64::from(ret))?;
            a.set_label(&mut done)?;
            a.pop(esi)?;
            a.ret_1(cleanup)
        })
    };
    let table = crate::FUNCTION_TABLE;
    assert_eq!(
        stub_shape_bytes(&unevaluated(RETURN, 8), A),
        Some(Native::DiscardedUnevaluated)
    );
    let good = evaluated(table, ARGUMENTS, RETURN, 12);
    assert_eq!(stub_shape_bytes(&good, A), Some(Native::DiscardedEvaluated));
    // Wrong call targets, argument slot, table, stack cleanup, base address,
    // or a truncated body.
    assert_eq!(stub_shape_bytes(&unevaluated(RETURN + 0x10, 8), A), None);
    assert_eq!(stub_shape_bytes(&unevaluated(RETURN, 4), A), None);
    assert_eq!(stub_shape_bytes(&unevaluated(RETURN, 8), A + 4), None);
    for bad in [
        evaluated(table, ARGUMENTS + 0x10, RETURN, 12),
        evaluated(table, ARGUMENTS, RETURN - 0x10, 12),
        evaluated(table + 4, ARGUMENTS, RETURN, 12),
        evaluated(table, ARGUMENTS, RETURN, 8),
    ] {
        assert_eq!(stub_shape_bytes(&bad, A), None);
    }
    assert_eq!(stub_shape_bytes(&good[..good.len() - 1], A), None);
}

#[test]
fn exported_state_round_trips_and_resumes_like_the_original() {
    let build = || {
        let mut b = B::default();
        let zero = b.real(0.0);
        b.define_global("g", REAL, zero);
        let l3 = log(&mut b, 3);
        let n = b.short(10);
        let s = b.call(op::SLEEP, VOID, &[n]);
        let root = b.call(op::BEGIN, VOID, &[l3, s]);
        b.script("loop", 0, VOID, root);
        b.build()
    };
    let (scenario, c, image) = (build(), catalog(), image());
    let mut vm = Vm::new(&scenario, &c, &image);
    let mut run = Run::new(Policy::Neutral);
    run.go(&mut vm, 5);
    let bytes = vm.export_state().encode();
    assert_eq!(VmState::decode(&bytes).unwrap().encode(), bytes);

    // The original goes on; a fresh VM given the state goes on identically.
    let mut resumed = Vm::new(&scenario, &c, &image);
    resumed.import_state(VmState::decode(&bytes).unwrap()).unwrap();
    let mut run2 = Run::new(Policy::Neutral);
    run.tick(&mut vm, 5, 30);
    run2.tick(&mut resumed, 5, 30);
    assert_eq!(vm.export_state().encode(), resumed.export_state().encode());
    assert_eq!(run2.logs(), run.logs().into_iter().filter(|l| l.0 >= 5).collect::<Vec<_>>());

    // Damage is a typed error.
    assert_eq!(VmState::decode(&bytes[..bytes.len() - 1]).unwrap_err(), StateError::Truncated);
    let mut more = bytes.clone();
    more.push(0);
    assert_eq!(VmState::decode(&more).unwrap_err(), StateError::Trailing);
}
