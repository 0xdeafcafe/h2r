//! Native C against the integer ext80 authority. No disc-derived fixtures.
use std::path::Path;
use std::sync::OnceLock;

use emit::compile::{self, Arch};
use x87::{
    F80,
    abi::{self, X87Abi},
};

const WRAPPER: &str = r#"
#undef H2_X87_NATIVE
#define H2_X87_NATIVE 1
#include "h2gen.h"
uint32_t n_arith(X87Abi *s, uint32_t op, uint32_t dst, uint32_t kind, uint64_t src) {
    return h2x87n_arith(s, op, dst, kind, src);
}
uint32_t n_unary(X87Abi *s, uint32_t op) { return h2x87n_unary(s, op); }
uint32_t n_load(X87Abi *s, uint64_t bits) { return h2x87n_ld_f64(s, bits); }
uint32_t n_copy(X87Abi *s, uint32_t i) { return h2x87n_ld_st(s, i); }
uint32_t n_load32(X87Abi *s, uint32_t b) { return h2x87n_ld_f32(s, b); }
uint32_t n_const(X87Abi *s, uint32_t i) { return h2x87n_ld_const(s, i); }
uint32_t n_st_st(X87Abi *s, uint32_t i, uint32_t pop) { return h2x87n_st_st(s, i, pop); }
uint32_t n_commit(X87Abi *s, uint16_t sw, uint32_t pop) { return h2x87n_store_commit(s, sw, pop); }
uint32_t n_store64(X87Abi *s, uint32_t i, uint64_t *v, uint16_t *sw) {
    return h2x87n_st_f64(s, i, v, sw);
}
uint32_t n_store(X87Abi *s, uint32_t i, uint32_t *v, uint16_t *sw) {
    return h2x87n_st_f32(s, i, v, sw);
}
uint32_t n_eligible(X87Abi *s) {
    double v; return h2x87n_mode(s) && h2x87n_get(s, h2x87n_phys(s, 0), &v);
}
/* Change host RC only for this call, restoring it even on fallback. */
uint32_t n_directed(X87Abi *s, uint32_t op) {
#if defined(__aarch64__)
    uint64_t old, next;
    __asm__ volatile("mrs %0, fpcr" : "=r"(old));
    next = old | (UINT64_C(1) << 22);
    __asm__ volatile("msr fpcr, %0" :: "r"(next));
    uint32_t code = h2x87n_arith(s, op, 0, 0, 1);
    __asm__ volatile("msr fpcr, %0" :: "r"(old));
    return code;
#elif defined(__x86_64__)
    uint32_t old, next;
    __asm__ volatile("stmxcsr %0" : "=m"(old));
    next = old | (1u << 13);
    __asm__ volatile("ldmxcsr %0" :: "m"(next));
    uint32_t code = h2x87n_arith(s, op, 0, 0, 1);
    __asm__ volatile("ldmxcsr %0" :: "m"(old));
    return code;
#endif
}
"#;

fn native() -> &'static libloading::Library {
    static LIB: OnceLock<libloading::Library> = OnceLock::new();
    LIB.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("native-x87");
        let src = dir.join("src");
        emit::write_module(&[], &src).unwrap();
        compile::install_header(
            &src,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/include/h2abi.h"),
        )
        .unwrap();
        let b = compile::build(&src, &dir, Arch::host(), 4).unwrap();
        let path = dir.join("libnative_x87.dylib");
        compile::host_dylib(
            &b,
            &src,
            &[("bridge.c", emit::host::BRIDGE), ("native.c", WRAPPER)],
            &path,
        )
        .unwrap();
        // SAFETY: built for this host with the reference bridge's ABI.
        unsafe {
            let lib = libloading::Library::new(path).unwrap();
            let install: libloading::Symbol<
                unsafe extern "C" fn(*const *const std::ffi::c_void, u32),
            > = lib.get(b"h2x87bridge_install").unwrap();
            let helpers = emit::host::x87_helpers();
            install(helpers.as_ptr(), helpers.len() as u32);
            lib
        }
    })
}

type Arith = unsafe extern "C" fn(&mut X87Abi, u32, u32, u32, u64) -> u32;
type Unary = unsafe extern "C" fn(&mut X87Abi, u32) -> u32;

fn state(a: F80, b: F80, cw: u16, top: u16) -> X87Abi {
    let mut s = X87Abi {
        control: cw,
        status: top << 11 | 0x4720,
        ..X87Abi::default()
    };
    s.set_st(0, a);
    s.set_st(1, b);
    s
}

fn next(r: &mut u64) -> u64 {
    *r ^= *r << 13;
    *r ^= *r >> 7;
    *r ^= *r << 17;
    *r
}

#[test]
fn native_arithmetic_matches_every_state_bit_and_return_code() {
    // SAFETY: wrapper has precisely Arith's C signature, library is retained.
    let call: libloading::Symbol<Arith> = unsafe { native().get(b"n_arith").unwrap() };
    let mut r = 0xcafef00d12345678;
    let edge = [
        F80::ZERO,
        F80::ZERO.neg(),
        F80::ONE,
        F80::ONE.neg(),
        F80::from_f64_bits(0x3ff0000000000001),
        F80::PI,
        F80::INFINITY,
        F80::INDEFINITE,
        F80::new(0x7fff, 0x8000000000000001),
        F80::new(0, 1),
        F80::new(0x7ffe, u64::MAX),
        F80::new(16383 - 401, 1 << 63),
        F80::new(16383 + 401, 1 << 63),
        F80::new(16383 - 400, 1 << 63),
        F80::new(16383 + 400, !0x7ff),
    ];
    for cw in [
        0x027f, 0x007f, 0x037f, 0x017f, 0x067f, 0x0a7f, 0x0e7f, 0x025f, 0x027e,
    ] {
        for i in 0..3000 {
            let values = |r: &mut u64| {
                let bits = next(r);
                let exp = 1023 - 400 + next(r) % 801;
                F80::from_f64_bits((bits & 0x800fffffffffffff) | exp << 52)
            };
            let a = if i < edge.len() * edge.len() {
                edge[i / edge.len()]
            } else {
                values(&mut r)
            };
            let b = if i < edge.len() * edge.len() {
                edge[i % edge.len()]
            } else {
                values(&mut r)
            };
            for op in 0..6 {
                for pop in [0, 0x100] {
                    for dst in [0, 1] {
                        let mut want = state(a, b, cw, (i & 7) as u16);
                        if i % 41 == 0 {
                            want.status |= 0x80;
                        }
                        if i % 43 == 0 {
                            want.tag |= 3 << (2 * want.phys(0));
                        }
                        let mut got = want;
                        let expected = abi::arith(&mut want, op | pop, dst, 0, 1 - dst as u64);
                        // SAFETY: valid state and wrapper signature.
                        let actual = unsafe { call(&mut got, op | pop, dst, 0, 1 - dst as u64) };
                        assert_eq!(
                            (got, actual),
                            (want, expected),
                            "cw={cw:x} i={i} op={op:x}/{pop:x} dst={dst} a={a:?} b={b:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn memory_sources_and_sqrt_match_reference() {
    // SAFETY: signatures exactly match the C wrappers.
    let arith: libloading::Symbol<Arith> = unsafe { native().get(b"n_arith").unwrap() };
    let unary: libloading::Symbol<Unary> = unsafe { native().get(b"n_unary").unwrap() };
    let mut r = 0x123456789abcdef;
    for i in 0..20000 {
        let bits = next(&mut r);
        let a = F80::from_f64_bits(
            (bits & 0x800fffffffffffff) | ((1023 - 400 + next(&mut r) % 801) << 52),
        );
        let cw = if i % 5 == 0 { 0x007f } else { 0x027f };
        for kind in 1..=4 {
            let src = next(&mut r);
            for op in 0..6 {
                let mut want = state(a, F80::ONE, cw, (i & 7) as u16);
                let mut got = want;
                let expected = abi::arith(&mut want, op, 0, kind, src);
                // SAFETY: matching signature and valid state.
                let actual = unsafe { arith(&mut got, op, 0, kind, src) };
                assert_eq!(
                    (got, actual),
                    (want, expected),
                    "source kind={kind} op={op} i={i}"
                );
            }
        }
        let mut want = state(a, F80::ONE, cw, (i & 7) as u16);
        let mut got = want;
        let expected = abi::unary(&mut want, 0);
        // SAFETY: matching signature and valid state.
        let actual = unsafe { unary(&mut got, 0) };
        assert_eq!((got, actual), (want, expected), "sqrt {a:?}");
    }
}

#[test]
fn transport_store_status_and_host_rounding_fallback() {
    // SAFETY: each type matches its wrapper's declaration.
    unsafe {
        let load: libloading::Symbol<unsafe extern "C" fn(&mut X87Abi, u64) -> u32> =
            native().get(b"n_load").unwrap();
        let copy: libloading::Symbol<Unary> = native().get(b"n_copy").unwrap();
        let store: libloading::Symbol<
            unsafe extern "C" fn(&X87Abi, u32, &mut u32, &mut u16) -> u32,
        > = native().get(b"n_store").unwrap();
        let directed: libloading::Symbol<Unary> = native().get(b"n_directed").unwrap();
        let eligible: libloading::Symbol<unsafe extern "C" fn(&mut X87Abi) -> u32> =
            native().get(b"n_eligible").unwrap();
        let mut r = 0x5555aaaa5555aaaa;
        for i in 0..10000 {
            let bits = next(&mut r);
            let mut want = state(F80::from_f64_bits(bits), F80::ONE, 0x027f, (i & 7) as u16);
            if i % 7 == 0 {
                want.tag = 0;
            }
            if i % 11 == 0 {
                want.status |= 0x80;
            }
            let mut got = want;
            assert_eq!(load(&mut got, bits), abi::ld_f64(&mut want, bits));
            assert_eq!(got, want);
            assert_eq!(copy(&mut got, 1), abi::ld_st(&mut want, 1));
            assert_eq!(got, want);
            let (expected, value) = abi::st_f32(&want, 0);
            let (mut v, mut sw) = (value.value, value.status);
            assert_eq!(store(&got, 0, &mut v, &mut sw), expected);
            if expected == abi::status::OK {
                assert_eq!((v, sw), (value.value, value.status));
            }
        }
        let a = F80::from_f64_bits(0x3ff0000000000001);
        let b = F80::from_f64_bits(0x3ca0000000000000);
        let mut want = state(a, b, 0x027f, 0);
        let mut got = want;
        assert_eq!(
            eligible(&mut got),
            1,
            "the ordinary case must exercise native code"
        );
        assert_eq!(directed(&mut got, 0), abi::arith(&mut want, 0, 0, 0, 1));
        assert_eq!(
            got, want,
            "host directed rounding must take the integer fallback"
        );
    }
}

#[cfg(feature = "oracle-tests")]
#[test]
fn native_values_match_the_independent_unicorn_oracle() {
    use iced_x86::code_asm::*;
    // Unicorn does not reliably set PC-dependent precision, sticky flags or
    // C1. Restrict its independent evidence to representable exact results
    // and transport/TOP/tags; the bit-exact differential tests above cover
    // all results and status bits, including inexact and fallback cases.
    let call: libloading::Symbol<Arith> = unsafe { native().get(b"n_arith").unwrap() };
    let unary: libloading::Symbol<Unary> = unsafe { native().get(b"n_unary").unwrap() };
    for cw in [0x007f, 0x027f, 0x037f] {
        for top in 0..8 {
            for (a, b) in [(4.0, 2.0), (-4.0, 2.0), (0.0, -2.0), (1.5, 0.5)] {
                for op in 0..7 {
                    let mut s = state(
                        F80::from_f64_bits(f64::to_bits(a)),
                        F80::from_f64_bits(f64::to_bits(b)),
                        cw,
                        top,
                    );
                    s.status &= 0x3800;
                    if op == 6 && a < 0.0 {
                        continue;
                    }
                    // 1.5's irrational sqrt needs a PC-aware oracle; see above.
                    if a == 1.5 && (op == 5 || op == 6) {
                        continue;
                    }
                    let mut input = oracle::CpuState::default();
                    input.x87.control = cw;
                    input.x87.status = s.status;
                    input.x87.tag = s.tag;
                    for i in 0..8 {
                        input.x87.st[i] = s.reg(s.phys(i as u32)).to_bytes();
                    }
                    let mut asm = CodeAssembler::new(32).unwrap();
                    match op {
                        0 => asm.fadd_2(st0, st1),
                        1 => asm.fsub_2(st0, st1),
                        2 => asm.fsubr_2(st0, st1),
                        3 => asm.fmul_2(st0, st1),
                        4 => asm.fdiv_2(st0, st1),
                        5 => asm.fdivr_2(st0, st1),
                        _ => asm.fsqrt(),
                    }
                    .unwrap();
                    let code = asm.assemble(input.eip as u64).unwrap();
                    let result = oracle::run(&code, input, &[]);
                    assert_eq!(result.stop, oracle::Stop::Completed);
                    // SAFETY: wrapper signatures match, library is retained.
                    let status = unsafe {
                        if op == 6 {
                            unary(&mut s, 0)
                        } else {
                            call(&mut s, op, 0, 0, 1)
                        }
                    };
                    assert_eq!(status, abi::status::OK);
                    assert_eq!(
                        s.reg(s.phys(0)).to_bytes(),
                        result.state.x87.st[0],
                        "cw={cw:x} top={top} op={op} a={a} b={b}"
                    );
                    assert_eq!(s.tag, result.state.x87.tag);
                    assert_eq!(s.status & 0x3800, result.state.x87.status & 0x3800);
                }
            }
        }
    }
}

#[test]
fn stack_transport_keeps_tags_pop_bytes_and_store_commit_protocol() {
    type Store64 = unsafe extern "C" fn(&X87Abi, u32, &mut u64, &mut u16) -> u32;
    // SAFETY: all signatures match the C wrappers; states remain valid Rust
    // objects even when their guest encodings/tags are deliberately unusual.
    unsafe {
        let load: libloading::Symbol<Unary> = native().get(b"n_load32").unwrap();
        let constant: libloading::Symbol<Unary> = native().get(b"n_const").unwrap();
        let copy: libloading::Symbol<Unary> = native().get(b"n_copy").unwrap();
        let store: libloading::Symbol<Store64> = native().get(b"n_store64").unwrap();
        let st_st: libloading::Symbol<unsafe extern "C" fn(&mut X87Abi, u32, u32) -> u32> =
            native().get(b"n_st_st").unwrap();
        let commit: libloading::Symbol<unsafe extern "C" fn(&mut X87Abi, u16, u32) -> u32> =
            native().get(b"n_commit").unwrap();
        let mut r = 0x1428571428571428;
        for i in 0..5000 {
            let mut initial = state(F80::ONE, F80::PI, 0x027f, (i & 7) as u16);
            initial.tag = next(&mut r) as u16;
            initial.status = next(&mut r) as u16;
            initial.control = next(&mut r) as u16;
            for reg in &mut initial.st {
                *reg = F80::new(next(&mut r) as u16, next(&mut r)).to_bytes();
            }
            let bits = next(&mut r) as u32;
            let which = i % 7;
            let reg = i % 8;
            let mut want = initial;
            let mut got = initial;
            assert_eq!(load(&mut got, bits), abi::ld_f32(&mut want, bits));
            assert_eq!(got, want);
            assert_eq!(constant(&mut got, which), abi::ld_const(&mut want, which));
            assert_eq!(got, want);
            assert_eq!(copy(&mut got, reg), abi::ld_st(&mut want, reg));
            assert_eq!(got, want);
            let (expected, converted) = abi::st_f64(&want, reg);
            let (mut value, mut sw) = (0, 0);
            assert_eq!(store(&got, reg, &mut value, &mut sw), expected);
            if expected == abi::status::OK {
                assert_eq!((value, sw), (converted.value, converted.status));
            }
            let pop = i & 1;
            assert_eq!(
                st_st(&mut got, reg, pop),
                abi::st_st(&mut want, reg, pop != 0)
            );
            assert_eq!(got, want);
            let sw = next(&mut r) as u16;
            assert_eq!(commit(&mut got, sw, pop), abi::status::OK);
            abi::store_commit(&mut want, sw, pop != 0);
            assert_eq!(
                got, want,
                "all physical bytes survive pop, including empty registers"
            );
        }
    }
}
