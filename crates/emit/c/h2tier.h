/* Sim tier support (crates/emit/src/sim.rs, docs/engine/sim-tier-experiment.md).
 * Pasted after `#include "h2gen.h"` into the one generated tier source. Not
 * derived from any disc.
 *
 * The contract is the looser one of docs/engine/sim-review.md section 4.2:
 * registers are C locals (SSA after clang's mem2reg), x87 values are binary64
 * locals, guest memory is one unchecked mapping at `base + address`, guest
 * calls are C calls. Anything the fast path cannot prove exact returns a
 * nonzero H2T_RARE_* code at once; the caller restores the arena and re-runs
 * the reference tier. Nothing here models MXCSR's sticky flags.
 *
 * Every floating-point operation is IEEE binary32 or binary64, compiled with
 * -ffp-contract=off and without fast-math (crates/emit/src/compile.rs). */
#ifndef H2TIER_H
#define H2TIER_H
#include <math.h>
#include <string.h>

#define H2T_OK 0u
#define H2T_RARE_ENTRY 1u  /* the FP environment is not the tick's (RN, PC=53, masked) */
#define H2T_RARE_FP 2u     /* an SSE result was a NaN */
#define H2T_RARE_X87 3u    /* an x87 value left the exact binary64 subset, a stack fault, or ZE */
#define H2T_RARE_CALL 4u   /* a call or jump to code outside the tier */
#define H2T_RARE_FAULT 5u  /* a guest exception (#DE, int3, ud2, misaligned movaps) */
#define H2T_RARE_STOP 6u   /* hlt, an unknown switch target, a descent error */
#define H2T_RARE_RETURN 7u /* a callee returned somewhere other than its continuation */

/* EFLAGS bits the tier keeps as separate locals: CF PF AF ZF SF DF OF, in
 * ir::Flag::ALL order. */
#define H2T_MODELLED 0xcd5u
static const unsigned h2t_flag_bit[7] = {0u, 2u, 4u, 6u, 7u, 10u, 11u};

/* What crosses a call between tier functions. A function loads what it
 * uses into locals on entry and stores what it wrote at every exit. */
typedef struct H2TState {
    h2u128 x[8];
    double fx[8];        /* x87 physical registers as binary64 */
    uint32_t r[8];
    uint32_t efr, fdr;   /* EFLAGS value and defined bits outside H2T_MODELLED */
    uint32_t ftop, fip, fdp;
    uint32_t fs_base, ret, rare_pc;
    uint16_t fsw, fcw, fop, fcs, fds; /* fsw without TOP */
    uint8_t f[7], d[7];  /* modelled flags and their defined bits */
    uint8_t fv[8], fw[8];/* x87 slot valid (not empty); written by the tier */
} H2TState;

#define H2T_RARE(code, pc) do { s->rare_pc = (pc); return (code); } while (0)

/* ---- memory: one unchecked mapping, the base a parameter ----------------- */

static inline uint8_t h2t_ld8(const uint8_t *b, uint32_t a) { return b[a]; }
static inline uint16_t h2t_ld16(const uint8_t *b, uint32_t a) { uint16_t v; memcpy(&v, b + a, 2); return v; }
static inline uint32_t h2t_ld32(const uint8_t *b, uint32_t a) { uint32_t v; memcpy(&v, b + a, 4); return v; }
static inline uint64_t h2t_ld64(const uint8_t *b, uint32_t a) { uint64_t v; memcpy(&v, b + a, 8); return v; }
static inline h2u128 h2t_ld128(const uint8_t *b, uint32_t a) { h2u128 v; memcpy(&v, b + a, 16); return v; }
static inline void h2t_st8(uint8_t *b, uint32_t a, uint8_t v) { b[a] = v; }
static inline void h2t_st16(uint8_t *b, uint32_t a, uint16_t v) { memcpy(b + a, &v, 2); }
static inline void h2t_st32(uint8_t *b, uint32_t a, uint32_t v) { memcpy(b + a, &v, 4); }
static inline void h2t_st64(uint8_t *b, uint32_t a, uint64_t v) { memcpy(b + a, &v, 8); }
static inline void h2t_st128(uint8_t *b, uint32_t a, h2u128 v) { memcpy(b + a, &v, 16); }

/* ---- SSE: binary32 on the host, a NaN result is a rare event ------------- */

static inline float h2t_f(uint32_t b) { float f; memcpy(&f, &b, 4); return f; }
static inline uint32_t h2t_b(float f) { uint32_t b; memcpy(&b, &f, 4); return b; }
static inline uint32_t h2t_lane(h2u128 v, unsigned i) { return (uint32_t)(v >> (32u * i)); }
static inline h2u128 h2t_lane0(h2u128 a, uint32_t r) { return (a & ~(h2u128)0xffffffffu) | (h2u128)r; }

#define H2T_SSE_ARITH(name, OP)                                                          \
    static inline int h2t_##name##ss(h2u128 *out, h2u128 a, uint32_t b) {                \
        float r = h2t_f(h2t_lane(a, 0)) OP h2t_f(b);                                    \
        if (r != r) return 0;                                                            \
        *out = h2t_lane0(a, h2t_b(r));                                                   \
        return 1;                                                                        \
    }                                                                                    \
    static inline int h2t_##name##ps(h2u128 *out, h2u128 a, h2u128 b) {                  \
        h2u128 o = 0;                                                                    \
        int ok = 1;                                                                      \
        for (unsigned i = 0; i < 4u; i++) {                                              \
            float r = h2t_f(h2t_lane(a, i)) OP h2t_f(h2t_lane(b, i));                   \
            ok &= r == r;                                                                \
            o |= (h2u128)h2t_b(r) << (32u * i);                                          \
        }                                                                                \
        *out = o;                                                                        \
        return ok;                                                                       \
    }
H2T_SSE_ARITH(add, +)
H2T_SSE_ARITH(sub, -)
H2T_SSE_ARITH(mul, *)
H2T_SSE_ARITH(div, /)
#undef H2T_SSE_ARITH

static inline int h2t_sqrtss(h2u128 *out, h2u128 a, uint32_t b) {
    float r = __builtin_sqrtf(h2t_f(b));
    if (r != r) return 0;
    *out = h2t_lane0(a, h2t_b(r));
    return 1;
}
static inline int h2t_sqrtps(h2u128 *out, h2u128 b) {
    h2u128 o = 0;
    int ok = 1;
    for (unsigned i = 0; i < 4u; i++) {
        float r = __builtin_sqrtf(h2t_f(h2t_lane(b, i)));
        ok &= r == r;
        o |= (h2u128)h2t_b(r) << (32u * i);
    }
    *out = o;
    return ok;
}
/* The x86 select: the second operand on a NaN or two zeros. Bits, not
 * values, so a NaN operand is passed through untouched. */
static inline uint32_t h2t_min1(uint32_t x, uint32_t y) { return h2t_f(x) < h2t_f(y) ? x : y; }
static inline uint32_t h2t_max1(uint32_t x, uint32_t y) { return h2t_f(x) > h2t_f(y) ? x : y; }
static inline h2u128 h2t_minss(h2u128 a, uint32_t b) { return h2t_lane0(a, h2t_min1(h2t_lane(a, 0), b)); }
static inline h2u128 h2t_maxss(h2u128 a, uint32_t b) { return h2t_lane0(a, h2t_max1(h2t_lane(a, 0), b)); }
static inline h2u128 h2t_minps(h2u128 a, h2u128 b) {
    h2u128 o = 0;
    for (unsigned i = 0; i < 4u; i++) o |= (h2u128)h2t_min1(h2t_lane(a, i), h2t_lane(b, i)) << (32u * i);
    return o;
}
static inline h2u128 h2t_maxps(h2u128 a, h2u128 b) {
    h2u128 o = 0;
    for (unsigned i = 0; i < 4u; i++) o |= (h2u128)h2t_max1(h2t_lane(a, i), h2t_lane(b, i)) << (32u * i);
    return o;
}
static inline uint32_t h2t_cmp1(uint32_t xb, uint32_t yb, uint8_t pred) {
    float x = h2t_f(xb), y = h2t_f(yb);
    int r;
    switch (pred & 7u) {
    case 0: r = x == y; break;
    case 1: r = x < y; break;
    case 2: r = x <= y; break;
    case 3: r = x != x || y != y; break;
    case 4: r = !(x == y); break;
    case 5: r = !(x < y); break;
    case 6: r = !(x <= y); break;
    default: r = x == x && y == y; break;
    }
    return r ? 0xffffffffu : 0u;
}
static inline h2u128 h2t_cmpss(h2u128 a, uint32_t b, uint8_t pred) {
    return h2t_lane0(a, h2t_cmp1(h2t_lane(a, 0), b, pred));
}
static inline h2u128 h2t_cmpps(h2u128 a, h2u128 b, uint8_t pred) {
    h2u128 o = 0;
    for (unsigned i = 0; i < 4u; i++) o |= (h2u128)h2t_cmp1(h2t_lane(a, i), h2t_lane(b, i), pred) << (32u * i);
    return o;
}
/* comiss/ucomiss: the EFLAGS image of ZF, PF, CF (MXCSR flags are not kept). */
static inline uint32_t h2t_comi(uint32_t ab, uint32_t bb) {
    float x = h2t_f(ab), y = h2t_f(bb);
    if (x != x || y != y) return 0x45u;
    if (x < y) return 0x01u;
    if (x == y) return 0x40u;
    return 0u;
}
static inline h2u128 h2t_cvtsi2ss(h2u128 a, uint32_t i) {
    return h2t_lane0(a, h2t_b((float)(int32_t)i));
}
static inline uint32_t h2t_f2i(float r) {
    if (r >= -2147483648.0f && r < 2147483648.0f) return (uint32_t)(int32_t)r;
    return 0x80000000u;
}
static inline uint32_t h2t_cvttss2si(uint32_t a) { return h2t_f2i(h2t_f(a)); }
static inline uint32_t h2t_cvtss2si(uint32_t a) { return h2t_f2i(__builtin_rintf(h2t_f(a))); }

/* ---- x87 as binary64 ------------------------------------------------------
 * The tier keeps an x87 value only while it is +-0 or a normal binary64 with
 * unbiased exponent in [-400, 400] (docs/lifter/x87-native.md has the proof
 * that PC=53 arithmetic on such values is binary64 arithmetic, and the PE/C1
 * reconstruction used here). Anything else is a rare event. */

static inline uint64_t h2t_bits64(double x) { uint64_t b; memcpy(&b, &x, 8); return b; }
static inline double h2t_dbl(uint64_t b) { double x; memcpy(&x, &b, 8); return x; }
static inline int h2t_x87_ok(double q) {
    uint64_t b = h2t_bits64(q) << 1;
    if (!b) return 1;
    unsigned e = (unsigned)(b >> 53);
    return e >= 1023u - 400u && e <= 1023u + 400u;
}
/* FLD m32 of a zero or normal: exact widening by bits (no host conversion). */
static inline int h2t_x87_f32(uint32_t b, double *out) {
    unsigned e = (b >> 23) & 0xffu;
    if (e == 0xffu || (!e && (b << 1))) return 0; /* inf, NaN (IE), denormal (DE) */
    uint64_t v = (uint64_t)(b >> 31) << 63;
    if (e) v |= ((uint64_t)(e + (1023u - 127u)) << 52) | ((uint64_t)(b & 0x7fffffu) << 29);
    *out = h2t_dbl(v);
    return 1;
}
static inline int h2t_x87_f64(uint64_t b, double *out) {
    double v = h2t_dbl(b);
    if (!h2t_x87_ok(v)) return 0;
    *out = v;
    return 1;
}
static inline uint16_t h2t_x87_status(uint16_t sw, int inexact, int up) {
    return (uint16_t)((sw & ~0x200u) | (inexact ? 0x20u : 0u) | (up ? 0x200u : 0u));
}
/* h2x87_v1_arith's ops: 0 add, 1 sub, 2 subr, 3 mul, 4 div, 5 divr; `a` is
 * the destination. */
static inline int h2t_x87_arith(unsigned op, double a, double b, double *out, uint16_t *sw) {
    double q, err;
    if (op == 2u || op == 5u) { double t = a; a = b; b = t; }
    if (op <= 2u) {
        if (op) b = -b;
        q = a + b;
        if (__builtin_fabs(a) < __builtin_fabs(b)) { double t = a; a = b; b = t; }
        err = b - (q - a); /* FastTwoSum: the exact signed error */
    } else if (op == 3u) {
        q = a * b;
        err = __builtin_fma(a, b, -q);
    } else {
        if (!(h2t_bits64(b) << 1)) return 0; /* ZE */
        q = a / b;
        err = __builtin_fma(-q, b, a);
        if (b < 0.0) err = -err;
    }
    if (!h2t_x87_ok(q)) return 0;
    *out = q;
    *sw = h2t_x87_status(*sw, err != 0.0, (q < 0.0 && err > 0.0) || (q > 0.0 && err < 0.0));
    return 1;
}
static inline int h2t_x87_sqrt(double a, double *out, uint16_t *sw) {
    uint64_t b = h2t_bits64(a);
    if ((b >> 63) && (b << 1)) return 0; /* IE */
    double q = __builtin_sqrt(a), err = __builtin_fma(q, q, -a);
    *out = q;
    *sw = h2t_x87_status(*sw, err != 0.0, err > 0.0);
    return 1;
}
/* FST m32: binary64 to binary32 away from the binary32 range edges. */
static inline int h2t_x87_st_f32(double a, uint16_t sw, uint32_t *out, uint16_t *nsw) {
    uint64_t b = h2t_bits64(a) & UINT64_C(0x7fffffffffffffff);
    if (b && (b < UINT64_C(0x3810000000000000) || b > UINT64_C(0x47efffffe0000000))) return 0;
    float q = (float)a;
    *out = h2t_b(q);
    double err = a - (double)q;
    *nsw = h2t_x87_status(sw, err != 0.0, (q < 0.0f && err > 0.0) || (q > 0.0f && err < 0.0));
    return 1;
}
/* 0 greater, 1 less, 2 equal (no NaN can reach a tier compare). */
static inline unsigned h2t_x87_rel(double a, double b) { return a > b ? 0u : a < b ? 1u : 2u; }
static inline unsigned h2t_x87_cc(unsigned rel) { return rel == 1u ? 0x100u : rel == 2u ? 0x4000u : 0u; }
static inline unsigned h2t_x87_eflags(unsigned rel) { return rel == 1u ? 0x01u : rel == 2u ? 0x40u : 0u; }

/* ---- the tier boundary ---------------------------------------------------- */

static inline int h2t_host_mode(void) {
#if defined(__aarch64__)
    uint64_t fpcr;
    __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
    return !(fpcr & UINT64_C(0xc09f00)); /* RN, no traps (FZ is bit 24) */
#elif defined(__x86_64__)
    uint32_t mxcsr;
    __asm__ volatile("stmxcsr %0" : "=m"(mxcsr));
    return (mxcsr & 0xffc0u) == 0x1f80u; /* RN, masked, no FTZ/DAZ */
#else
    return 0;
#endif
}
/* The guest's SSE environment the tier assumes: every exception masked,
 * round to nearest, no FTZ (DAZ is reserved on the Pentium III). */
static inline int h2t_sse_mode(const CpuAbi *c) { return (c->mxcsr & 0xffc0u) == 0x1f80u && h2t_host_mode(); }

static inline int h2t_x87_get(const X87Abi *x, unsigned p, double *out) {
    uint64_t m;
    uint16_t se;
    memcpy(&m, x->st[p], 8);
    memcpy(&se, x->st[p] + 8, 2);
    unsigned e = se & 0x7fffu;
    uint64_t b = (uint64_t)(se & 0x8000u) << 48;
    if (!e && !m) { *out = h2t_dbl(b); return 1; }
    if (e < 16383u - 400u || e > 16383u + 400u || !(m >> 63) || (m & 0x7ffu)) return 0;
    b |= (uint64_t)(e - (16383u - 1023u)) << 52;
    b |= (m >> 11) & UINT64_C(0xfffffffffffff);
    *out = h2t_dbl(b);
    return 1;
}
static inline void h2t_x87_put(X87Abi *x, unsigned p, double v) {
    uint64_t b = h2t_bits64(v);
    unsigned e = (unsigned)((b >> 52) & 0x7ffu);
    uint64_t m = e ? ((b << 11) | (UINT64_C(1) << 63)) : 0;
    uint16_t se = (uint16_t)((b >> 48) & 0x8000u);
    if (e) se |= (uint16_t)(e + (16383u - 1023u));
    memcpy(x->st[p], &m, 8);
    memcpy(x->st[p] + 8, &se, 2);
}
/* The tick's x87 environment: PC=53, RN, every exception masked, nothing
 * pending, and every live register in the exact subset with a tag that
 * matches its class. */
static inline int h2t_x87_enter(H2TState *s, const X87Abi *x) {
    if ((x->control & 0xf3fu) != 0x23fu || (x->status & 0x80u)) return 0;
    for (unsigned p = 0; p < 8u; p++) {
        unsigned tag = (x->tag >> (2u * p)) & 3u;
        s->fw[p] = 0;
        s->fv[p] = tag != 3u;
        s->fx[p] = 0.0;
        if (tag == 3u) continue;
        double v;
        if (!h2t_x87_get(x, p, &v) || tag != (v == 0.0 ? 1u : 0u)) return 0;
        s->fx[p] = v;
    }
    s->ftop = (x->status >> 11) & 7u;
    s->fsw = (uint16_t)(x->status & ~0x3800u);
    s->fcw = x->control;
    s->fop = x->opcode;
    s->fcs = x->instruction_selector;
    s->fds = x->data_selector;
    s->fip = x->instruction_pointer;
    s->fdp = x->data_pointer;
    return 1;
}
static inline void h2t_x87_leave(const H2TState *s, X87Abi *x) {
    uint16_t tag = 0;
    for (unsigned p = 0; p < 8u; p++) {
        if (s->fv[p] || s->fw[p]) h2t_x87_put(x, p, s->fx[p]);
        unsigned t = s->fv[p] ? (s->fx[p] == 0.0 ? 1u : 0u) : 3u;
        tag |= (uint16_t)(t << (2u * p));
    }
    x->tag = tag;
    x->status = (uint16_t)((s->fsw & ~0x3800u) | ((s->ftop & 7u) << 11));
    x->opcode = s->fop;
    x->instruction_selector = s->fcs;
    x->data_selector = s->fds;
    x->instruction_pointer = s->fip;
    x->data_pointer = s->fdp;
}
static inline void h2t_flags_enter(H2TState *s, const CpuAbi *c) {
    for (unsigned i = 0; i < 7u; i++) {
        s->f[i] = (uint8_t)((c->eflags >> h2t_flag_bit[i]) & 1u);
        s->d[i] = (uint8_t)((c->eflags_defined >> h2t_flag_bit[i]) & 1u);
    }
    s->efr = c->eflags & ~H2T_MODELLED;
    s->fdr = c->eflags_defined & ~H2T_MODELLED;
}
static inline void h2t_flags_leave(const H2TState *s, CpuAbi *c) {
    uint32_t fl = s->efr, fd = s->fdr;
    for (unsigned i = 0; i < 7u; i++) {
        fl |= (uint32_t)(s->f[i] & 1u) << h2t_flag_bit[i];
        fd |= (uint32_t)(s->d[i] & 1u) << h2t_flag_bit[i];
    }
    c->eflags = fl;
    c->eflags_defined = fd;
}
#endif
