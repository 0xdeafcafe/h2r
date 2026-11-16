/* Guarded native binary64 x87 lowering. See docs/lifter/x87-native.md for the
 * value/range proof and the PE/C1 reconstruction. All other cases call the
 * ext80 authority. Architectural registers always retain their 10 bytes. */
#ifndef H2X87_NATIVE_H
#define H2X87_NATIVE_H
#if H2_X87_NATIVE
#include <math.h>
/* The host-mode guard must precede arithmetic even when host traps are
 * enabled. Strict FP (FENV_ACCESS) would forbid speculation, but LLVM
 * never inlines a strict-FP function into the non-strict generated
 * handlers, so every operation became a real call. Instead the operands
 * pass through an empty volatile asm placed after every guard: no FP
 * operation can be evaluated before the checks have passed, and past them
 * the host is round-to-nearest with traps masked, the environment the
 * non-strict compiler assumes. Bit moves (h2x87n_double) are not FP. */
#if defined(__aarch64__)
#define H2X87N_BARRIER(x) __asm__ volatile("" : "+w"(x))
#elif defined(__x86_64__)
#define H2X87N_BARRIER(x) __asm__ volatile("" : "+x"(x))
#else
#define H2X87N_BARRIER(x) __asm__ volatile("" : "+m"(x))
#endif
#define H2X87N_FAST __attribute__((always_inline))

static inline unsigned h2x87n_phys(const X87Abi *s, unsigned i) {
    return ((s->status >> 11) + i) & 7u;
}
static inline int h2x87n_live(const X87Abi *s, unsigned p) {
    return ((s->tag >> (2u * p)) & 3u) != 3u;
}
static inline void h2x87n_tag(X87Abi *s, unsigned p, unsigned t) {
    s->tag = (uint16_t)((s->tag & ~(3u << (2u * p))) | (t << (2u * p)));
}
static inline void h2x87n_pop(X87Abi *s) {
    unsigned p = h2x87n_phys(s, 0);
    h2x87n_tag(s, p, 3);
    s->status = (uint16_t)((s->status & ~0x3800u) | (((p + 1u) & 7u) << 11));
}
static inline uint64_t h2x87n_bits(double x) {
    uint64_t b; memcpy(&b, &x, 8); return b;
}
static inline double h2x87n_double(uint64_t b) {
    double x; memcpy(&x, &b, 8); return x;
}
/* Exactly widen a normal binary64 or a signed zero. */
static inline void h2x87n_put(X87Abi *s, unsigned p, uint64_t b) {
    unsigned e = (unsigned)((b >> 52) & 0x7ffu);
    uint64_t m = e ? ((b << 11) | (UINT64_C(1) << 63)) : 0;
    uint16_t se = (uint16_t)((b >> 48) & 0x8000u);
    if (e) se |= (uint16_t)(e + (16383 - 1023));
    memcpy(s->st[p], &m, 8); memcpy(s->st[p] + 8, &se, 2);
    h2x87n_tag(s, p, e ? 0 : 1);
}
/* Accept only canonical zero or a normal exact double with unbiased
 * exponent in [-400,400]. Low ext80 precision bits must actually be zero;
 * PC=53 alone says nothing about loads/constants/previous PC=64 results. */
static inline H2X87N_FAST int h2x87n_get(const X87Abi *s, unsigned p, double *out) {
    uint64_t m; uint16_t se;
    if (!h2x87n_live(s, p)) return 0;
    memcpy(&m, s->st[p], 8); memcpy(&se, s->st[p] + 8, 2);
    unsigned e = se & 0x7fffu;
    uint64_t b = (uint64_t)(se & 0x8000u) << 48;
    if (!e && !m) { *out = h2x87n_double(b); return 1; }
    if (e < 16383 - 400 || e > 16383 + 400 ||
        !(m >> 63) || (m & 0x7ffu)) return 0;
    b |= (uint64_t)(e - (16383 - 1023)) << 52;
    b |= (m >> 11) & UINT64_C(0xfffffffffffff);
    *out = h2x87n_double(b); return 1;
}
static inline H2X87N_FAST int h2x87n_host_mode(void) {
#if defined(__aarch64__)
    uint64_t fpcr;
    __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
    return !(fpcr & UINT64_C(0xc09f00)); /* RC and exception trap enables */
#elif defined(__x86_64__)
    uint32_t mxcsr;
    __asm__ volatile("stmxcsr %0" : "=m"(mxcsr));
    return (mxcsr & 0x7f80u) == 0x1f80u; /* RN and all exceptions masked */
#else
    return 0;
#endif
}
static inline H2X87N_FAST int h2x87n_mode(const X87Abi *s) {
    /* Every mask set, PC=53, RC=nearest, no pending exception. Check the
     * host rounding mode too; this reads FPCR/MXCSR. All
     * nonzero intermediates in the arithmetic proof are normal, so host
     * flush-to-zero cannot affect them. */
    return (s->control & 0xf3fu) == 0x23fu && !(s->status & 0x80u) &&
           h2x87n_host_mode();
}
static inline uint16_t h2x87n_status(uint16_t sw, int inexact, int up) {
    return (uint16_t)((sw & ~0x200u) | (inexact ? 0x20u : 0) | (up ? 0x200u : 0));
}
static inline uint32_t h2x87n_ld_f64(X87Abi *s, uint64_t b) {
    unsigned e = (unsigned)((b >> 52) & 0x7ffu), p = h2x87n_phys(s, 7);
    if ((s->status & 0x80u) || h2x87n_live(s, p) || e == 0x7ffu ||
        (!e && (b << 1))) return h2x87_v1_ld_f64(s, b);
    s->status = (uint16_t)((s->status & ~0x3a00u) | (p << 11));
    h2x87n_put(s, p, b); return H2X87_OK;
}
static inline uint32_t h2x87n_ld_f32(X87Abi *s, uint32_t b) {
    unsigned e = (b >> 23) & 0xffu;
    if (e == 0xffu || (!e && (b << 1))) return h2x87_v1_ld_f32(s, b);
    /* Bit widening avoids host float conversion, including host FTZ. */
    uint64_t v = (uint64_t)(b >> 31) << 63;
    if (e) v |= ((uint64_t)(e + (1023 - 127)) << 52) | ((uint64_t)(b & 0x7fffffu) << 29);
    return h2x87n_ld_f64(s, v);
}
/* FILD m16/m32: every such integer is exact in binary64 (normal or +0),
 * so this is FLD of that double; the conversion is exact, flag-free and
 * independent of the rounding mode. Overflow and pending #MF fall back. */
static inline uint32_t h2x87n_ld_i32(X87Abi *s, int32_t v) {
    uint64_t b = 0;
    if (v) {
        uint32_t m = v < 0 ? 0u - (uint32_t)v : (uint32_t)v;
        unsigned top = 31u - (unsigned)__builtin_clz(m);
        b = ((uint64_t)(v < 0) << 63) | ((uint64_t)(1023u + top) << 52) |
            (((uint64_t)m << (52u - top)) & UINT64_C(0xfffffffffffff));
    }
    unsigned p = h2x87n_phys(s, 7);
    if ((s->status & 0x80u) || h2x87n_live(s, p)) return h2x87_v1_ld_i32(s, v);
    s->status = (uint16_t)((s->status & ~0x3a00u) | (p << 11));
    h2x87n_put(s, p, b); return H2X87_OK;
}
static inline uint32_t h2x87n_ld_i16(X87Abi *s, int16_t v) {
    unsigned p = h2x87n_phys(s, 7);
    if ((s->status & 0x80u) || h2x87n_live(s, p)) return h2x87_v1_ld_i16(s, v);
    return h2x87n_ld_i32(s, v);
}
static inline uint32_t h2x87n_ld_const(X87Abi *s, uint32_t which) {
    if (which <= 1) return h2x87n_ld_f64(s, which ? 0 : UINT64_C(0x3ff0000000000000));
    return h2x87_v1_ld_const(s, which);
}
static inline uint32_t h2x87n_ld_st(X87Abi *s, uint32_t i) {
    unsigned from = h2x87n_phys(s, i), to = h2x87n_phys(s, 7);
    if ((s->status & 0x80u) || !h2x87n_live(s, from) || h2x87n_live(s, to))
        return h2x87_v1_ld_st(s, i);
    memcpy(s->st[to], s->st[from], 10);
    /* Reference transport reclassifies tags even for inconsistent input. */
    uint64_t m; uint16_t se;
    memcpy(&m, s->st[to], 8); memcpy(&se, s->st[to] + 8, 2);
    unsigned e = se & 0x7fffu;
    h2x87n_tag(s, to, !e && !m ? 1 : (e && e != 0x7fffu && (m >> 63) ? 0 : 2));
    s->status = (uint16_t)((s->status & ~0x3a00u) | (to << 11));
    return H2X87_OK;
}
static inline uint32_t h2x87n_store_commit(X87Abi *s, uint16_t sw, uint32_t pop) {
    s->status = (uint16_t)((sw & ~0x3800u) | (s->status & 0x3800u));
    if (pop) h2x87n_pop(s);
    return H2X87_OK;
}
static inline uint32_t h2x87n_st_st(X87Abi *s, uint32_t dst, uint32_t pop) {
    /* Keep the uncommon arbitrary-tag/empty-stack cases at the authority. */
    double x; unsigned p = h2x87n_phys(s, 0);
    if ((s->status & 0x80u) || !h2x87n_get(s, p, &x)) return h2x87_v1_st_st(s, dst, pop);
    h2x87n_put(s, h2x87n_phys(s, dst), h2x87n_bits(x));
    s->status &= (uint16_t)~0x200u;
    if (pop) h2x87n_pop(s);
    return H2X87_OK;
}
static inline uint32_t h2x87n_xch(X87Abi *s, uint32_t i) {
    /* Tag classification and stack faults belong to the reference. */
    return h2x87_v1_xch(s, i);
}
static inline H2X87N_FAST uint32_t h2x87n_arith(X87Abi *s, uint32_t op, uint32_t dst, uint32_t kind, uint64_t src) {
    double a, b, q, err;
    unsigned p = h2x87n_phys(s, dst);
    if (!h2x87n_mode(s) || !h2x87n_get(s, p, &a)) goto slow;
    if (kind == 0) {
        if (!h2x87n_get(s, h2x87n_phys(s, (unsigned)src), &b)) goto slow;
    } else if (kind == 1) {
        uint32_t v = (uint32_t)src; unsigned e = (v >> 23) & 0xffu;
        if (!e || e == 0xffu) goto slow;
        uint64_t bits = ((uint64_t)(v >> 31) << 63) | ((uint64_t)(e + 896) << 52) | ((uint64_t)(v & 0x7fffffu) << 29);
        b = h2x87n_double(bits);
    } else if (kind == 2) {
        unsigned e = (unsigned)((src >> 52) & 0x7ffu);
        if (e < 1023 - 400 || e > 1023 + 400) goto slow;
        b = h2x87n_double(src);
    } else if (kind == 3) b = (double)(int16_t)src; /* exact, no flags */
    else if (kind == 4) b = (double)(int32_t)src;
    else goto slow;
    H2X87N_BARRIER(a);
    H2X87N_BARRIER(b);
    unsigned code = op & 0xffu;
    if (code == 2 || code == 5) { double t = a; a = b; b = t; }
    if (code <= 2) {
        if (code) b = -b;
        q = a + b;
        /* FastTwoSum, magnitude-ordered: exact signed error exact-q. */
        if (__builtin_fabs(a) < __builtin_fabs(b)) { double t = a; a = b; b = t; }
        err = b - (q - a);
    } else if (code == 3) {
        q = a * b;
        err = __builtin_fma(a, b, -q);
    } else {
        if (!(h2x87n_bits(b) << 1)) goto slow; /* +-0: no FP compare */
        q = a / b;
        err = __builtin_fma(-q, b, a);
        if (b < 0.0) err = -err;
    }
    h2x87n_put(s, p, h2x87n_bits(q));
    s->status = h2x87n_status(s->status, err != 0.0,
        (q < 0.0 && err > 0.0) || (q > 0.0 && err < 0.0));
    if (op & 0x100u) h2x87n_pop(s);
    return H2X87_OK;
slow:
    return h2x87_v1_arith(s, op, dst, kind, src);
}
static inline H2X87N_FAST uint32_t h2x87n_unary(X87Abi *s, uint32_t op) {
    double a;
    unsigned p = h2x87n_phys(s, 0);
    if (op != 0 || !h2x87n_mode(s) || !h2x87n_get(s, p, &a) ||
        (h2x87n_bits(a) >> 63 && h2x87n_bits(a) << 1))
        return h2x87_v1_unary(s, op);
    H2X87N_BARRIER(a);
    double q = __builtin_sqrt(a), err = __builtin_fma(q, q, -a);
    h2x87n_put(s, p, h2x87n_bits(q));
    s->status = h2x87n_status(s->status, err != 0.0, err > 0.0);
    return H2X87_OK;
}
static inline H2X87N_FAST uint32_t h2x87n_st_f64(X87Abi *s, uint32_t i, uint64_t *value, uint16_t *sw) {
    double a;
    if (!h2x87n_mode(s) || !h2x87n_get(s, h2x87n_phys(s, i), &a))
        return h2x87_v1_st_f64(s, i, value, sw);
    *value = h2x87n_bits(a); *sw = (uint16_t)(s->status & ~0x200u);
    return H2X87_OK;
}
static inline H2X87N_FAST uint32_t h2x87n_st_f32(X87Abi *s, uint32_t i, uint32_t *value, uint16_t *sw) {
    double a;
    if (!h2x87n_mode(s) || !h2x87n_get(s, h2x87n_phys(s, i), &a)) goto slow;
    /* Exclude the binary32 underflow/overflow ranges (including rounding
     * across their boundaries), preserving ext80's range and sticky bits. */
    uint64_t b = h2x87n_bits(a) & UINT64_C(0x7fffffffffffffff);
    if (b && (b < UINT64_C(0x3810000000000000) || b > UINT64_C(0x47efffffe0000000))) goto slow;
    H2X87N_BARRIER(a);
    float q = (float)a;
    memcpy(value, &q, 4);
    double err = a - (double)q;
    *sw = h2x87n_status(s->status, err != 0.0,
        (q < 0.0f && err > 0.0) || (q > 0.0f && err < 0.0));
    return H2X87_OK;
slow:
    return h2x87_v1_st_f32(s, i, value, sw);
}
#undef H2X87N_FAST
#else
#define h2x87n_ld_f32 h2x87_v1_ld_f32
#define h2x87n_ld_f64 h2x87_v1_ld_f64
#define h2x87n_ld_i16 h2x87_v1_ld_i16
#define h2x87n_ld_i32 h2x87_v1_ld_i32
#define h2x87n_ld_const h2x87_v1_ld_const
#define h2x87n_ld_st h2x87_v1_ld_st
#define h2x87n_st_f32 h2x87_v1_st_f32
#define h2x87n_st_f64 h2x87_v1_st_f64
#define h2x87n_store_commit h2x87_v1_store_commit
#define h2x87n_st_st h2x87_v1_st_st
#define h2x87n_arith h2x87_v1_arith
#define h2x87n_unary h2x87_v1_unary
#define h2x87n_xch h2x87_v1_xch
#endif
#endif
