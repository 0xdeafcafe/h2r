//! The interpreter's side of [`Intrinsic::X87`](crate::Intrinsic::X87):
//! each [`X87Op`] as the same `h2x87_v1_*` call the emitter makes, with the
//! status handling of the [`crate::x87`] module docs.
//!
//! [`helpers`] is the one binding to `crates/recompiler/x87`; [`X87Op::Mark`] is IR
//! state and needs no helper.

use crate::interp::Cpu;
use crate::x87::{GUEST_CS, GUEST_DS, X87Op, status};
use crate::{Fault, StopReason};

/// Why an x87 op did not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Fault(Fault),
    Stop(StopReason),
}

fn check(st: u32) -> Result<(), Outcome> {
    match st & status::MASK {
        status::OK => Ok(()),
        status::MF => Err(Outcome::Fault(Fault::FpError)),
        status::UNMODELLED => Err(Outcome::Stop(StopReason::Unmodelled)),
        s => panic!("x87 helper returned status {s:#x} where it cannot"),
    }
}

/// Execute one x87 op at instruction `pc`.
pub(crate) fn exec(cpu: &mut Cpu, pc: u32, op: X87Op, args: &[u128]) -> Result<Vec<u128>, Outcome> {
    use helpers as h;
    let s = &mut cpu.x87;
    let arg = |i: usize| args[i];
    Ok(match op {
        X87Op::Mark { fop, mem } => {
            s.instruction_pointer = pc;
            s.instruction_selector = GUEST_CS;
            s.opcode = fop & 0x7ff;
            if mem {
                s.data_pointer = arg(0) as u32;
                s.data_selector = GUEST_DS;
            }
            vec![]
        }
        X87Op::Load(f) => {
            check(h::ld(s, f, arg(0)))?;
            vec![]
        }
        X87Op::LoadConst(c) => {
            check(h::ld_const(s, c as u32))?;
            vec![]
        }
        X87Op::LoadReg(i) => {
            check(h::ld_st(s, i as u32))?;
            vec![]
        }
        X87Op::StoreConvert { to, src } => {
            let (st, value, sw) = h::st(s, to, src as u32);
            let write = if st & status::MASK == status::SUPPRESS {
                false
            } else {
                check(st)?;
                true
            };
            vec![value, sw as u128, write as u128]
        }
        X87Op::StoreCommit { pop } => {
            let write = arg(1) != 0;
            check(h::store_commit(s, arg(0) as u16, pop && write))?;
            vec![]
        }
        X87Op::StoreReg { dst, pop } => {
            check(h::st_st(s, dst as u32, pop))?;
            vec![]
        }
        X87Op::Arith { op, dst, src, pop } => {
            let sel = op as u32 | if pop { 0x100 } else { 0 };
            let (dst_i, bits) = match src {
                crate::x87::X87Src::St(i) => (dst as u32, i as u64),
                _ => (0, arg(0) as u64),
            };
            check(h::arith(s, sel, dst_i, src.kind(), bits))?;
            vec![]
        }
        X87Op::Unary(u) => {
            check(h::unary(s, u as u32))?;
            vec![]
        }
        X87Op::Cmp { kind, src, pop, to_flags } => {
            let bits = match src {
                crate::x87::X87Src::St(i) => i as u64,
                _ => arg(0) as u64,
            };
            let st = h::cmp(s, kind as u32, src.kind(), bits, pop as u32, to_flags);
            let suppressed = st & status::MASK == status::SUPPRESS;
            if !suppressed {
                check(st)?;
            }
            vec![(st & 0x00ff_ffff) as u128, suppressed as u128]
        }
        X87Op::Xam => {
            check(h::xam(s))?;
            vec![]
        }
        X87Op::Xch(i) => {
            check(h::xch(s, i as u32))?;
            vec![]
        }
        X87Op::Free { reg, pop } => {
            check(h::free(s, reg as u32, pop))?;
            vec![]
        }
        X87Op::IncStp => {
            check(h::adj_top(s, 1))?;
            vec![]
        }
        X87Op::DecStp => {
            check(h::adj_top(s, -1))?;
            vec![]
        }
        X87Op::Cmov(i) => {
            check(h::cmov(s, i as u32, arg(0) != 0))?;
            vec![]
        }
        X87Op::LoadCw => {
            check(h::ldcw(s, arg(0) as u16))?;
            vec![]
        }
        X87Op::Clex => {
            check(h::clex(s))?;
            vec![]
        }
        X87Op::Init => {
            check(h::init(s))?;
            vec![]
        }
        X87Op::SaveImage(e) => {
            let (st, img) = h::save_image(s, e as u32);
            check(st)?;
            split(&img[..e.bytes() as usize], e.chunks())
        }
        X87Op::SaveCommit(e) => {
            check(h::save_commit(s, e as u32))?;
            vec![]
        }
        X87Op::LoadEnv(e) => {
            let img = join(args, e.chunks());
            check(h::load_env(s, e as u32, &img))?;
            vec![]
        }
        X87Op::Ftol2 => {
            let (st, out) = h::ftol2(s);
            check(st)?;
            vec![out as u128]
        }
    })
}

/// An image as chunk values, little-endian.
fn split(img: &[u8], chunks: &[crate::Width]) -> Vec<u128> {
    let mut off = 0;
    chunks
        .iter()
        .map(|w| {
            let n = w.bytes() as usize;
            let mut b = [0u8; 16];
            b[..n].copy_from_slice(&img[off..off + n]);
            off += n;
            u128::from_le_bytes(b)
        })
        .collect()
}

/// Chunk values back into a 108-byte image buffer.
fn join(args: &[u128], chunks: &[crate::Width]) -> [u8; 108] {
    let mut img = [0u8; 108];
    let mut off = 0;
    for (v, w) in args.iter().zip(chunks) {
        let n = w.bytes() as usize;
        img[off..off + n].copy_from_slice(&v.to_le_bytes()[..n]);
        off += n;
    }
    img
}

/// The binding to `crates/recompiler/x87`'s `h2x87_v1_*` semantics (`x87::abi`), one
/// function per helper, on the interpreter's `X87` (the same fields as
/// `X87Abi`, copied in and out around each call).
pub(crate) mod helpers {
    use crate::interp::X87;
    use crate::x87::X87Format;
    use x87::abi::{self, X87Abi, status};

    fn to_abi(s: &X87) -> X87Abi {
        X87Abi {
            st: s.st,
            control: s.control,
            status: s.status,
            tag: s.tag,
            opcode: s.opcode,
            instruction_pointer: s.instruction_pointer,
            data_pointer: s.data_pointer,
            instruction_selector: s.instruction_selector,
            data_selector: s.data_selector,
        }
    }

    fn from_abi(s: &mut X87, a: &X87Abi) {
        s.st = a.st;
        s.control = a.control;
        s.status = a.status;
        s.tag = a.tag;
        s.opcode = a.opcode;
        s.instruction_pointer = a.instruction_pointer;
        s.data_pointer = a.data_pointer;
        s.instruction_selector = a.instruction_selector;
        s.data_selector = a.data_selector;
    }

    /// Run `f` on an `X87Abi` copy of `s`, then copy it back.
    fn with<T>(s: &mut X87, f: impl FnOnce(&mut X87Abi) -> T) -> T {
        let mut a = to_abi(s);
        let r = f(&mut a);
        from_abi(s, &a);
        r
    }

    fn bytes10(bits: u128) -> [u8; 10] {
        bits.to_le_bytes()[..10].try_into().unwrap()
    }

    fn bits10(b: [u8; 10]) -> u128 {
        let mut v = [0u8; 16];
        v[..10].copy_from_slice(&b);
        u128::from_le_bytes(v)
    }

    pub fn ld(s: &mut X87, f: X87Format, bits: u128) -> u32 {
        with(s, |a| match f {
            X87Format::F32 => abi::ld_f32(a, bits as u32),
            X87Format::F64 => abi::ld_f64(a, bits as u64),
            X87Format::F80 => abi::ld_f80(a, bytes10(bits)),
            X87Format::I16 => abi::ld_int(a, bits as u16 as i16 as i64),
            X87Format::I32 => abi::ld_int(a, bits as u32 as i32 as i64),
            X87Format::I64 => abi::ld_int(a, bits as u64 as i64),
            X87Format::Bcd => abi::ld_bcd(a, bytes10(bits)),
        })
    }
    pub fn ld_const(s: &mut X87, which: u32) -> u32 {
        with(s, |a| abi::ld_const(a, which))
    }
    pub fn ld_st(s: &mut X87, i: u32) -> u32 {
        with(s, |a| abi::ld_st(a, i))
    }
    /// (status, bits to write, status word for the commit).
    pub fn st(s: &X87, f: X87Format, i: u32) -> (u32, u128, u16) {
        let a = to_abi(s);
        fn out<T>(r: (u32, abi::Store<T>), bits: impl FnOnce(T) -> u128) -> (u32, u128, u16) {
            (r.0, bits(r.1.value), r.1.status)
        }
        match f {
            X87Format::F32 => out(abi::st_f32(&a, i), |v| v as u128),
            X87Format::F64 => out(abi::st_f64(&a, i), |v| v as u128),
            X87Format::F80 => out(abi::st_f80(&a, i), bits10),
            X87Format::I16 => out(abi::st_i16(&a, i), |v| v as u16 as u128),
            X87Format::I32 => out(abi::st_i32(&a, i), |v| v as u32 as u128),
            X87Format::I64 => out(abi::st_i64(&a, i), |v| v as u64 as u128),
            X87Format::Bcd => out(abi::st_bcd(&a, i), bits10),
        }
    }
    pub fn store_commit(s: &mut X87, sw: u16, pop: bool) -> u32 {
        with(s, |a| abi::store_commit(a, sw, pop));
        status::OK
    }
    pub fn st_st(s: &mut X87, dst: u32, pop: bool) -> u32 {
        with(s, |a| abi::st_st(a, dst, pop))
    }
    pub fn arith(s: &mut X87, op: u32, dst: u32, src_kind: u32, src: u64) -> u32 {
        with(s, |a| abi::arith(a, op, dst, src_kind, src))
    }
    pub fn unary(s: &mut X87, op: u32) -> u32 {
        with(s, |a| abi::unary(a, op))
    }
    pub fn cmp(s: &mut X87, kind: u32, src_kind: u32, src: u64, pop: u32, to_flags: bool) -> u32 {
        with(s, |a| abi::cmp(a, kind, src_kind, src, pop, to_flags))
    }
    pub fn xam(s: &mut X87) -> u32 {
        with(s, abi::xam)
    }
    pub fn xch(s: &mut X87, i: u32) -> u32 {
        with(s, |a| abi::xch(a, i))
    }
    pub fn free(s: &mut X87, i: u32, pop: bool) -> u32 {
        with(s, |a| abi::free(a, i, pop))
    }
    pub fn adj_top(s: &mut X87, delta: i32) -> u32 {
        with(s, |a| abi::adj_top(a, delta))
    }
    pub fn cmov(s: &mut X87, i: u32, take: bool) -> u32 {
        with(s, |a| abi::cmov(a, i, take))
    }
    pub fn ldcw(s: &mut X87, cw: u16) -> u32 {
        with(s, |a| abi::ldcw(a, cw))
    }
    pub fn clex(s: &mut X87) -> u32 {
        with(s, abi::clex);
        status::OK
    }
    pub fn init(s: &mut X87) -> u32 {
        with(s, abi::init);
        status::OK
    }
    pub fn save_image(s: &X87, op: u32) -> (u32, [u8; 108]) {
        (status::OK, abi::save_image(&to_abi(s), op))
    }
    pub fn save_commit(s: &mut X87, op: u32) -> u32 {
        with(s, |a| abi::save_commit(a, op));
        status::OK
    }
    pub fn load_env(s: &mut X87, op: u32, img: &[u8; 108]) -> u32 {
        with(s, |a| abi::load_env(a, op, img))
    }
    pub fn ftol2(s: &mut X87) -> (u32, u64) {
        let mut out = 0;
        let st = with(s, |a| abi::ftol2(a, &mut out));
        (st, out)
    }
}
