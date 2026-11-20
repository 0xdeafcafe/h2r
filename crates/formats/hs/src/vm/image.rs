//! What the VM reads from the owner's executable at load time.
//!
//! Native evaluators are bound by **evaluate address**, not by name: builtin
//! names only come from map tokens, while the descriptor's evaluate pointer is
//! in the XBE. Several builtins share one evaluator and branch on their own
//! index (arithmetic at `0x00208310` jumps on `index - 7`), so the VM does the
//! same. Nothing here is compiled-in table data: [`VmImage::from_xbe`] checks
//! the constants the evaluators use against the executable and finds the
//! retail-discarded builtins by their instruction shape.
use super::value::HsType;
use crate::{Catalog, Result, invalid};
use Arg::*;
use Register::{EAX, ECX, EDX, ESI, ESP};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use std::collections::BTreeMap;

/// The VM's own implementation of a builtin evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Native {
    /// `0x00207c20`
    Begin,
    /// `0x00207cf0`
    BeginRandom,
    /// `0x00207ef0`
    If,
    /// `0x00208070`
    Set,
    /// `0x002081e0`, `and` when the builtin index is 5, else `or`.
    AndOr,
    /// `0x00208310`, `+ - * / min max` by `index - 7`.
    Arithmetic,
    /// `0x002084c0`, `!=` when the index is 14, else `=`.
    Equal,
    /// `0x00208570`, `> < >= <=` by `index - 15`.
    Compare,
    /// `0x002087e0`
    Sleep,
    /// `0x00208750`
    SleepForever,
    /// `0x002089b0`; its index (`0x15`) is special-cased by wake.
    SleepUntil,
    /// `0x00208c40`
    Wake,
    /// `0x00208e10`: evaluate, then format through the console inspector.
    Inspect,
    /// `0x00208f10`: object cast through the type-mask table at `0x004456d4`.
    ObjectCast,
    /// `0x002a0c40`
    Not,
    /// `0x002a0c90`
    Pin,
    /// `0x002a29d0`
    RandomRange,
    /// `0x002a2a50`
    RealRandomRange,
    /// Retail stub that returns 0 without evaluating its arguments.
    DiscardedUnevaluated,
    /// Retail stub that evaluates its arguments, then returns 0 (`print`).
    DiscardedEvaluated,
}

impl Native {
    /// Evaluators the VM implements, by evaluate address.
    pub const BY_ADDRESS: [(u32, Native); 18] = [
        (0x0020_7c20, Native::Begin),
        (0x0020_7cf0, Native::BeginRandom),
        (0x0020_7ef0, Native::If),
        (0x0020_8070, Native::Set),
        (0x0020_81e0, Native::AndOr),
        (0x0020_8310, Native::Arithmetic),
        (0x0020_84c0, Native::Equal),
        (0x0020_8570, Native::Compare),
        (0x0020_87e0, Native::Sleep),
        (0x0020_8750, Native::SleepForever),
        (0x0020_89b0, Native::SleepUntil),
        (0x0020_8c40, Native::Wake),
        (0x0020_8e10, Native::Inspect),
        (0x0020_8f10, Native::ObjectCast),
        (0x002a_0c40, Native::Not),
        (0x002a_0c90, Native::Pin),
        (0x002a_29d0, Native::RandomRange),
        (0x002a_2a50, Native::RealRandomRange),
    ];
    pub fn at(address: u32) -> Option<Native> {
        Self::BY_ADDRESS
            .iter()
            .find(|(a, _)| *a == address)
            .map(|&(_, n)| n)
    }
}

/// The builtin index the evaluator checks for `sleep_until`
/// (`0x00209c80` compares node `+2` with `0x15`).
pub const SLEEP_UNTIL_INDEX: u16 = 0x15;
/// Argument evaluator and return helper, the targets the stub shapes call.
pub const ARGUMENTS: u32 = 0x0020_9d50;
pub const RETURN: u32 = 0x0020_9ae0;

/// Object-type masks the cast evaluator tests, `0x004456d4[index - 23]`.
/// `unit` (index 24) reads entry 1: bipeds and vehicles (bits 0 and 1).
pub const OBJECT_CAST_MASKS: u32 = 0x0044_56d4;
/// Per-type byte sizes handed to `repe cmpsb` by `=`.
pub const TYPE_SIZES: u32 = 0x0044_5710;
/// `1/30` single used to scale authored ticks by the tick rate.
pub const TICK_SCALE: u32 = 0x0045_dc6c;
/// `1/65535` single used by `real_random_range` (so the top seed value reaches `high`).
pub const RANDOM_SCALE: u32 = 0x0045_dc50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmImage {
    /// Builtin index -> retail-discarded stub kind.
    pub(crate) discarded: BTreeMap<u16, Native>,
    /// Object-type masks by `index - 23`, for the object cast evaluator.
    pub(crate) cast_masks: Vec<u16>,
}

impl VmImage {
    /// For tests and hosts that build a catalog by hand: no stubs, and the
    /// `unit` mask (bipeds and vehicles) the retail table holds at entry 1.
    pub fn without_stubs() -> Self {
        Self {
            discarded: BTreeMap::new(),
            cast_masks: vec![0xffff, 0b11],
        }
    }

    /// Mark a retail-discarded builtin as supplied by the host engine.
    pub fn use_engine_builtin(&mut self, index: u16) {
        self.discarded.remove(&index);
    }

    pub fn discarded(&self) -> &BTreeMap<u16, Native> {
        &self.discarded
    }

    /// Check the constants the native evaluators assume, and classify every
    /// builtin whose evaluate body is one of the two retail stub shapes.
    pub fn from_xbe(x: &xbe::Xbe<'_>, catalog: &Catalog) -> Result<Self> {
        let f32_at =
            |a| -> Result<f32> { Ok(f32::from_le_bytes(x.bytes_at(a, 4)?.try_into().unwrap())) };
        if f32_at(TICK_SCALE)? != 1.0 / 30.0 || f32_at(RANDOM_SCALE)? != 1.0 / 65535.0 {
            return Err(invalid(
                "unsupported XBE: HaloScript time/random constants differ",
            ));
        }
        let sizes = x.bytes_at(TYPE_SIZES, 2 * HsType::COUNT as usize)?;
        for t in 0..HsType::COUNT {
            let size = u16::from_le_bytes([sizes[t as usize * 2], sizes[t as usize * 2 + 1]]);
            if size != u16::from(HsType(t).size()) {
                return Err(invalid(format!(
                    "unsupported XBE: type {t} size {size} differs from the VM's"
                )));
            }
        }
        let cast_masks = x
            .bytes_at(OBJECT_CAST_MASKS, 12)?
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        for &(address, native) in &Native::BY_ADDRESS {
            if !catalog.functions.iter().any(|f| f.evaluate == address) {
                return Err(invalid(format!(
                    "unsupported XBE: no builtin uses {native:?} at {address:#x}"
                )));
            }
        }
        let mut discarded = BTreeMap::new();
        for (i, f) in catalog.functions.iter().enumerate() {
            if f.evaluate == 0 || Native::at(f.evaluate).is_some() {
                continue;
            }
            if let Some(kind) = stub_shape(x, f.evaluate)? {
                discarded.insert(i as u16, kind);
            }
        }
        Ok(Self {
            discarded,
            cast_masks,
        })
    }
}

/// One expected operand of a decoded instruction.
#[derive(Clone, Copy)]
enum Arg {
    R(Register),
    /// `[base + index*scale + disp]`, `size` bytes (0: any, for `lea`).
    M(Register, Register, u32, u32, usize),
    /// Immediate.
    I(u64),
    /// Near branch or call to an absolute target.
    To(u32),
    /// Near branch to the instruction `n` places later in the shape.
    Ahead(usize),
}
type Shape = &'static [(Mnemonic, &'static [Arg])];

const NO: Register = Register::None;

/// `mov ecx,[esp+8]; xor eax,eax; call RETURN; ret 0Ch`: returns 0 at
/// once, never evaluating the arguments.
const UNEVALUATED: Shape = &[
    (Mnemonic::Mov, &[R(ECX), M(ESP, NO, 1, 8, 4)]),
    (Mnemonic::Xor, &[R(EAX), R(EAX)]),
    (Mnemonic::Call, &[To(RETURN)]),
    (Mnemonic::Ret, &[I(12)]),
];
/// The standard fixed-signature prologue (descriptor from the builtin
/// table, parameter count and types pushed for `ARGUMENTS`), then
/// `test eax,eax; je done; xor eax,eax; mov ecx,esi; call RETURN;
/// done: pop esi; ret 0Ch`, as `print` (`0x002ab6c0`) is. The table
/// operand is checked against [`crate::FUNCTION_TABLE`].
const EVALUATED: Shape = &[
    (Mnemonic::Movsx, &[R(EAX), M(ESP, NO, 1, 4, 2)]),
    (
        Mnemonic::Mov,
        &[R(EAX), M(NO, EAX, 4, crate::FUNCTION_TABLE, 4)],
    ),
    (Mnemonic::Mov, &[R(ECX), M(ESP, NO, 1, 12, 4)]),
    (Mnemonic::Push, &[R(ESI)]),
    (Mnemonic::Mov, &[R(ESI), M(ESP, NO, 1, 12, 4)]),
    (Mnemonic::Push, &[R(ECX)]),
    (Mnemonic::Lea, &[R(EDX), M(EAX, NO, 1, 14, 0)]),
    (Mnemonic::Movsx, &[R(EAX), M(EAX, NO, 1, 12, 2)]),
    (Mnemonic::Push, &[R(EDX)]),
    (Mnemonic::Push, &[R(EAX)]),
    (Mnemonic::Push, &[R(ESI)]),
    (Mnemonic::Call, &[To(ARGUMENTS)]),
    (Mnemonic::Test, &[R(EAX), R(EAX)]),
    (Mnemonic::Je, &[Ahead(4)]),
    (Mnemonic::Xor, &[R(EAX), R(EAX)]),
    (Mnemonic::Mov, &[R(ECX), R(ESI)]),
    (Mnemonic::Call, &[To(RETURN)]),
    (Mnemonic::Pop, &[R(ESI)]),
    (Mnemonic::Ret, &[I(12)]),
];

fn operand_matches(i: &Instruction, n: u32, want: Arg, ahead: Option<u32>) -> bool {
    match (i.op_kind(n), want) {
        (OpKind::Register, R(r)) => i.op_register(n) == r,
        (OpKind::Memory, M(base, index, scale, disp, size)) => {
            i.memory_base() == base
                && i.memory_index() == index
                && (index == NO || i.memory_index_scale() == scale)
                && i.memory_displacement32() == disp
                && i.segment_prefix() == NO
                && (size == 0 || i.memory_size().size() == size)
        }
        (
            OpKind::Immediate8 | OpKind::Immediate16 | OpKind::Immediate32 | OpKind::Immediate8to32,
            I(v),
        ) => i.immediate(n) == v,
        (OpKind::NearBranch32, To(t)) => i.near_branch32() == t,
        (OpKind::NearBranch32, Ahead(_)) => Some(i.near_branch32()) == ahead,
        _ => false,
    }
}

/// Decode `code` at `address` and compare instruction by instruction.
fn matches(code: &[u8], address: u32, shape: Shape) -> bool {
    let decoded: Vec<Instruction> =
        Decoder::with_ip(32, code, u64::from(address), DecoderOptions::NONE)
            .into_iter()
            .take_while(|i| !i.is_invalid())
            .take(shape.len())
            .collect();
    decoded.len() == shape.len()
        && decoded
            .iter()
            .zip(shape)
            .enumerate()
            .all(|(k, (i, (m, args)))| {
                let ahead = args.iter().find_map(|a| match a {
                    Ahead(n) => decoded.get(k + n).map(Instruction::ip32),
                    _ => None,
                });
                i.mnemonic() == *m
                    && i.op_count() as usize == args.len()
                    && args
                        .iter()
                        .enumerate()
                        .all(|(n, &a)| operand_matches(i, n as u32, a, ahead))
            })
}

/// Classify a builtin's evaluate body as one of the two retail stub shapes,
/// by decoded instructions and their call targets.
fn stub_shape(x: &xbe::Xbe<'_>, address: u32) -> Result<Option<Native>> {
    let code = x
        .bytes_at(address, 64)
        .or_else(|_| x.bytes_at(address, 16))
        .unwrap_or(&[]);
    Ok(stub_shape_bytes(code, address))
}

/// Classify evaluator bytes at `address` (see [`stub_shape`]).
pub(crate) fn stub_shape_bytes(code: &[u8], address: u32) -> Option<Native> {
    if matches(code, address, UNEVALUATED) {
        Some(Native::DiscardedUnevaluated)
    } else if matches(code, address, EVALUATED) {
        Some(Native::DiscardedEvaluated)
    } else {
        None
    }
}
