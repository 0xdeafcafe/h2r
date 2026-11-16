//! C for [`Intrinsic::X87`]: one `h2x87_v1_*` call per op, through the
//! `H2X87*` macros in `c/h2gen.h`.
//!
//! A call site is the helper call wrapped in `H2X87(pc, ...)`, which on a
//! non-OK status records it and jumps to the handler's one `h2_x87_fail`
//! block (#MF as event 14, UNMODELLED as stop 12), so the slow path costs
//! each site two stores and a branch. Memory operands are ordinary
//! `H2_LD`/`H2_ST` accesses emitted from the IR's own `Load`/`Store`
//! statements around these calls.
//!
//! FIP/FOP/FDP/FCS/FDS are handler locals (`H2X87_LOCALS`) that the
//! `Mark` op assigns and the epilogue writes back; within a block clang
//! drops every assignment but the last. They are spilled before
//! `save_image` and reloaded after the helpers that write them.

use ir::x87::{X87Env, X87Format, X87Op, X87Src};
use ir::{Intrinsic, IrFunction, Stmt, Temp, Value};

use crate::c::{ty, val};

/// Whether `f` has any x87 op (and so needs the x87 locals, fail block and
/// epilogue).
pub fn uses_x87(f: &IrFunction) -> bool {
    fn any(stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            Stmt::Intrinsic {
                op: Intrinsic::X87(_),
                ..
            } => true,
            Stmt::If {
                then, otherwise, ..
            } => any(then) || any(otherwise),
            Stmt::Repeat { body, .. } => any(body),
            _ => false,
        })
    }
    f.insns().any(|i| any(&i.stmts))
}

/// `T tN = 0;` for each result.
fn declare(dsts: &[Temp]) -> String {
    dsts.iter()
        .map(|t| format!("{} t{} = 0;", ty(t.width), t.id))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The helper's integer pointer type for a store result of `f`.
fn int_ptr(f: X87Format) -> Option<&'static str> {
    match f {
        X87Format::I16 => Some("int16_t"),
        X87Format::I32 => Some("int32_t"),
        X87Format::I64 => Some("int64_t"),
        _ => None,
    }
}

/// The `src` argument of `arith`/`cmp`.
fn src_bits(src: X87Src, args: &[Value]) -> String {
    match src {
        X87Src::St(i) => format!("{i}u"),
        _ => format!("(uint64_t){}", val(&args[0])),
    }
}

/// Copy chunk values to/from the image buffer `i_`.
fn chunk_copies(env: X87Env, vals: &[String], to_buffer: bool) -> String {
    let mut off = 0;
    let mut out = Vec::new();
    for (v, w) in vals.iter().zip(env.chunks()) {
        let n = w.bytes();
        out.push(if to_buffer {
            format!("memcpy(i_ + {off}, &{v}, {n});")
        } else {
            format!("memcpy(&{v}, i_ + {off}, {n});")
        });
        off += n;
    }
    out.join(" ")
}

/// The C lines for one x87 op of the instruction at `pc`.
pub fn stmt(op: X87Op, args: &[Value], dsts: &[Temp], pc: u32) -> Vec<String> {
    let s = "&cpu->x87";
    let a = |i: usize| val(&args[i]);
    let call = |c: String| format!("H2X87({pc:#x}u, H2_PERF_X87({c}));");
    match op {
        X87Op::Mark { fop, mem } => vec![if mem {
            format!("H2X87_MARKM({pc:#x}u, {fop:#x}u, {});", a(0))
        } else {
            format!("H2X87_MARK({pc:#x}u, {fop:#x}u);")
        }],
        X87Op::Load(f) => vec![call(match f {
            X87Format::F32 | X87Format::F64 => format!("h2x87n_ld_{}({s}, {})", f.suffix(), a(0)),
            X87Format::I16 | X87Format::I32 => {
                format!(
                    "h2x87n_ld_{}({s}, ({}){})",
                    f.suffix(),
                    int_ptr(f).unwrap(),
                    a(0)
                )
            }
            X87Format::F80 | X87Format::Bcd => format!(
                "h2x87i_ld80({s}, {}, {}u)",
                a(0),
                (f == X87Format::Bcd) as u32
            ),
            f => format!(
                "h2x87_v1_ld_{}({s}, ({}){})",
                f.suffix(),
                int_ptr(f).unwrap(),
                a(0)
            ),
        })],
        X87Op::LoadConst(c) => vec![call(format!("h2x87n_ld_const({s}, {}u)", c as u32))],
        X87Op::LoadReg(i) => vec![call(format!("h2x87n_ld_st({s}, {i}u)"))],
        X87Op::StoreConvert { to, src } => {
            let (v, sw, w) = (dsts[0].id, dsts[1].id, dsts[2].id);
            let c = match to {
                X87Format::F80 | X87Format::Bcd => {
                    format!(
                        "h2x87i_st80({s}, {src}u, {}u, &t{v}, &t{sw})",
                        (to == X87Format::Bcd) as u32
                    )
                }
                f => match int_ptr(f) {
                    Some(p) => format!(
                        "h2x87_v1_st_{}({s}, {src}u, ({p} *)&t{v}, &t{sw})",
                        f.suffix()
                    ),
                    None => format!("h2x87n_st_{}({s}, {src}u, &t{v}, &t{sw})", f.suffix()),
                },
            };
            vec![
                declare(dsts),
                format!("H2X87_ST({pc:#x}u, t{w}, H2_PERF_X87({c}));"),
            ]
        }
        X87Op::StoreCommit { pop } => {
            let p = if pop { a(1) } else { "0u".into() };
            vec![call(format!("h2x87n_store_commit({s}, {}, {p})", a(0)))]
        }
        X87Op::StoreReg { dst, pop } => {
            vec![call(format!("h2x87n_st_st({s}, {dst}u, {}u)", pop as u32))]
        }
        X87Op::Arith { op, dst, src, pop } => {
            let sel = op as u32 | if pop { 0x100 } else { 0 };
            let dst = if matches!(src, X87Src::St(_)) { dst } else { 0 };
            vec![call(format!(
                "h2x87n_arith({s}, {sel:#x}u, {dst}u, {}u, {})",
                src.kind(),
                src_bits(src, args)
            ))]
        }
        X87Op::Unary(u) => vec![call(format!("h2x87n_unary({s}, {}u)", u as u32))],
        X87Op::Cmp {
            kind,
            src,
            pop,
            to_flags,
        } => {
            let (p, sup) = (dsts[0].id, dsts[1].id);
            let c = format!(
                "h2x87_v1_cmp({s}, {}u, {}u, {}, {pop}u, {}u)",
                kind as u32,
                src.kind(),
                src_bits(src, args),
                to_flags as u32
            );
            vec![
                declare(dsts),
                format!("H2X87_CMP({pc:#x}u, t{p}, t{sup}, H2_PERF_X87({c}));"),
            ]
        }
        X87Op::Xam => vec![call(format!("h2x87_v1_xam({s})"))],
        X87Op::Xch(i) => vec![call(format!("h2x87n_xch({s}, {i}u)"))],
        X87Op::Free { reg, pop } => {
            vec![call(format!("h2x87_v1_free({s}, {reg}u, {}u)", pop as u32))]
        }
        X87Op::IncStp => vec![call(format!("h2x87_v1_adj_top({s}, 1)"))],
        X87Op::DecStp => vec![call(format!("h2x87_v1_adj_top({s}, -1)"))],
        X87Op::Cmov(i) => vec![call(format!("h2x87_v1_cmov({s}, {i}u, {})", a(0)))],
        X87Op::LoadCw => vec![call(format!("h2x87_v1_ldcw({s}, {})", a(0)))],
        X87Op::Clex => vec![call(format!("h2x87_v1_clex({s})"))],
        X87Op::Init => vec![
            call(format!("h2x87_v1_init({s})")),
            "H2X87_RELOAD();".into(),
        ],
        X87Op::SaveImage(e) => {
            let names: Vec<String> = dsts.iter().map(|t| format!("t{}", t.id)).collect();
            vec![
                declare(dsts),
                format!(
                    "{{ uint8_t i_[112] = {{0}}; H2X87_SPILL(); {} {} }}",
                    call(format!("h2x87_v1_save_image({s}, {}u, i_)", e as u32)),
                    chunk_copies(e, &names, false)
                ),
            ]
        }
        X87Op::SaveCommit(e) => {
            let mut v = vec![call(format!("h2x87_v1_save_commit({s}, {}u)", e as u32))];
            if e == X87Env::Save {
                v.push("H2X87_RELOAD();".into());
            }
            v
        }
        X87Op::LoadEnv(e) => {
            // Constants need an lvalue to copy from.
            let mut lines = Vec::new();
            let names: Vec<String> = args
                .iter()
                .enumerate()
                .map(|(k, v)| {
                    let n = format!("e{k}_");
                    lines.push(format!("{} {n} = {};", ty(v.width()), val(v)));
                    n
                })
                .collect();
            vec![
                format!(
                    "{{ {} uint8_t i_[112] = {{0}}; {} {} }}",
                    lines.join(" "),
                    chunk_copies(e, &names, true),
                    call(format!("h2x87_v1_load_env({s}, {}u, i_)", e as u32))
                ),
                "H2X87_RELOAD();".into(),
            ]
        }
        X87Op::Ftol2 => vec![
            declare(dsts),
            call(format!("h2x87_v1_ftol2({s}, &t{})", dsts[0].id)),
        ],
    }
}
