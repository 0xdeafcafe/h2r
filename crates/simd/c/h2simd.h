/* h2simd.h: bit-exact, MXCSR-exact reference semantics for the Pentium III's
 * SSE1 floating point, for the generated code. The C twin of crates/simd's
 * src/lib.rs, written line for line after it; the two give bit-identical
 * results (crates/simd/tests/differential.rs). Semantics: docs/lifter/simd.md.
 *
 * Header-only C11, integer soft-float only: no host floating point, no
 * floating-point environment, no undefined behaviour. Compiles cleanly with
 * -std=c11 -Wall -Wextra -Werror (and -pedantic).
 *
 * API: functions taking MXCSR OR the sticky exception flags they detect into
 * *mxcsr and return 0, or 1 for an unmasked exception (#XM), in which case
 * *out is not written. The rcp/rsqrt functions take no MXCSR and return the
 * result. Scalar (ss) forms keep lanes 1-3 of a. DAZ is ignored. */
#ifndef H2SIMD_H
#define H2SIMD_H
#include <stdint.h>

__extension__ typedef unsigned __int128 h2u128;

#define H2S_IE 0x01u
#define H2S_DE 0x02u
#define H2S_ZE 0x04u
#define H2S_OE 0x08u
#define H2S_UE 0x10u
#define H2S_PE 0x20u
#define H2S_MASK_SHIFT 7
#define H2S_RC_SHIFT 13
#define H2S_FTZ 0x8000u
#define H2S_DEFAULT_NAN 0xffc00000u
#define H2S_INT_INDEFINITE 0x80000000u
#define H2S__PRE (H2S_IE | H2S_DE | H2S_ZE)
#define H2S__QUIET 0x00400000u
#define H2S__UM (H2S_UE << H2S_MASK_SHIFT)
#define H2S__OM (H2S_OE << H2S_MASK_SHIFT)

/* An element result: the masked-response value and the flags detected. */
typedef struct { uint32_t r, f; } h2s__res;
static inline h2s__res h2s__mk(uint32_t r, uint32_t f) { h2s__res x; x.r = r; x.f = f; return x; }

/* ---- classification ---- */
static inline int h2s__is_nan(uint32_t a) { return (a & 0x7fffffffu) > 0x7f800000u; }
static inline int h2s__is_snan(uint32_t a) { return h2s__is_nan(a) && !(a & H2S__QUIET); }
static inline int h2s__is_inf(uint32_t a) { return (a & 0x7fffffffu) == 0x7f800000u; }
static inline int h2s__is_zero(uint32_t a) { return (a & 0x7fffffffu) == 0; }
static inline int h2s__is_denormal(uint32_t a) { return !(a & 0x7f800000u) && (a & 0x007fffffu); }
static inline uint32_t h2s__de2(uint32_t a, uint32_t b) {
    return (h2s__is_denormal(a) || h2s__is_denormal(b)) ? H2S_DE : 0;
}
static inline h2s__res h2s__nan2(uint32_t a, uint32_t b) {
    uint32_t r = h2s__is_nan(a) ? (a | H2S__QUIET) : (b | H2S__QUIET);
    return h2s__mk(r, (h2s__is_snan(a) || h2s__is_snan(b)) ? H2S_IE : 0);
}

static inline int h2s__clz32(uint32_t x) { /* x != 0 */
    int n = 0;
#if defined(__GNUC__) || defined(__clang__)
    n = __builtin_clz(x);
#else
    while (!(x & 0x80000000u)) { x <<= 1; n++; }
#endif
    return n;
}
static inline int h2s__clz64(uint64_t x) { /* x != 0 */
    int n = 0;
#if defined(__GNUC__) || defined(__clang__)
    n = __builtin_clzll(x);
#else
    while (!(x & 0x8000000000000000ull)) { x <<= 1; n++; }
#endif
    return n;
}

/* Unpacks a finite non-zero value as m * 2^(e - 150), m with bit 23 set. */
static inline void h2s__unpack_norm(uint32_t a, int32_t *e, uint32_t *m) {
    int32_t exp = (int32_t)((a >> 23) & 0xff);
    uint32_t frac = a & 0x007fffffu;
    if (exp == 0) {
        int32_t shift = h2s__clz32(frac) - 8;
        *e = 1 - shift;
        *m = frac << shift;
    } else {
        *e = exp;
        *m = frac | 0x00800000u;
    }
}

static inline uint32_t h2s__shift_right_jam32(uint32_t a, uint32_t dist) {
    if (dist == 0) return a;
    return dist < 31 ? (a >> dist) | (uint32_t)((a << (32 - dist)) != 0) : (uint32_t)(a != 0);
}
static inline uint64_t h2s__shift_right_jam64(uint64_t a, uint32_t dist) {
    if (dist == 0) return a;
    if (dist < 64) return (a >> dist) | (uint64_t)((a << (64 - dist)) != 0);
    return (uint64_t)(a != 0);
}

/* ---- rounding ---- */
/* Rounds sig * 2^(exp - 156) (sig below 2^31, bit 30 the leading bit of a
 * normal result, bits 0-6 the round bits). Tininess after rounding. */
static inline h2s__res h2s__round_pack(uint32_t mx, uint32_t sign, int32_t exp, uint32_t sig) {
    uint32_t rc = (mx >> H2S_RC_SHIFT) & 3;
    int near_even = rc == 0;
    uint32_t inc;
    uint32_t flags = 0, round_bits;
#ifdef H2SIMD_TEST_MUTATION
    /* Test-only fault injection (tests/mutation.rs): round up as round down. */
    if (rc == 2) rc = 1;
#endif
    switch (rc) {
    case 0: inc = 0x40; break;
    case 1: inc = sign ? 0x7f : 0; break;
    case 2: inc = sign ? 0 : 0x7f; break;
    default: inc = 0; break;
    }
    round_bits = sig & 0x7f;
    if ((uint32_t)exp >= 0xfd) {
        if (exp < 0) {
            int tiny = exp < -1 || sig + inc < 0x80000000u;
            /* Unmasked underflow traps before denormalisation: PE says
             * whether rounding to 24 bits (unbounded exponent) was inexact.
             * FTZ does not apply. The value is never stored. */
            if (tiny && !(mx & H2S__UM)) return h2s__mk(sign << 31, H2S_UE | (round_bits ? H2S_PE : 0));
            if (tiny && (mx & H2S_FTZ)) return h2s__mk(sign << 31, H2S_UE | H2S_PE);
            sig = h2s__shift_right_jam32(sig, (uint32_t)0 - (uint32_t)exp);
            exp = 0;
            round_bits = sig & 0x7f;
            if (tiny && round_bits != 0) flags |= H2S_UE;
        } else if (exp > 0xfd || sig + inc >= 0x80000000u) {
            /* Unmasked overflow: PE only for an inexact 24-bit rounding. */
            if (!(mx & H2S__OM)) return h2s__mk(sign << 31, H2S_OE | (round_bits ? H2S_PE : 0));
            return h2s__mk(((sign << 31) | 0x7f800000u) - (uint32_t)(inc == 0), H2S_OE | H2S_PE);
        }
    }
    if (round_bits != 0) flags |= H2S_PE;
    sig = (sig + inc) >> 7;
    if (near_even && round_bits == 0x40) sig &= ~1u;
    if (sig == 0) exp = 0;
    return h2s__mk((sign << 31) + ((uint32_t)exp << 23) + sig, flags);
}

/* Normalizes and rounds m * 2^q (0 < m < 2^63). */
static inline h2s__res h2s__norm_round_pack(uint32_t mx, uint32_t sign, int32_t q, uint64_t m) {
    int32_t s = h2s__clz64(m) - 1;
    uint32_t sig;
    m <<= s;
    sig = (uint32_t)(m >> 32) | (uint32_t)((uint32_t)m != 0);
    return h2s__round_pack(mx, sign, q - s + 32 + 156, sig);
}

static inline int h2s__rc_down(uint32_t mx) { return ((mx >> H2S_RC_SHIFT) & 3) == 1; }

/* ---- per-element operations ---- */
static inline h2s__res h2s__add1(uint32_t mx, uint32_t a, uint32_t b_in, int negate) {
    uint32_t b, flags;
    h2s__res r;
    if (h2s__is_nan(a) || h2s__is_nan(b_in)) return h2s__nan2(a, b_in);
    b = negate ? b_in ^ 0x80000000u : b_in;
    if (h2s__is_inf(a)) {
        if (h2s__is_inf(b) && ((a ^ b) >> 31) != 0) return h2s__mk(H2S_DEFAULT_NAN, H2S_IE);
        return h2s__mk(a, h2s__de2(a, b));
    }
    if (h2s__is_inf(b)) return h2s__mk(b, h2s__de2(a, b));
    flags = h2s__de2(a, b);
    if (h2s__is_zero(a) && h2s__is_zero(b)) {
        uint32_t z = a == b ? a : h2s__rc_down(mx) ? 0x80000000u : 0;
        return h2s__mk(z, flags);
    }
    if (h2s__is_zero(a)) {
        int32_t e; uint32_t m;
        h2s__unpack_norm(b, &e, &m);
        r = h2s__norm_round_pack(mx, b >> 31, e - 150, m);
    } else if (h2s__is_zero(b)) {
        int32_t e; uint32_t m;
        h2s__unpack_norm(a, &e, &m);
        r = h2s__norm_round_pack(mx, a >> 31, e - 150, m);
    } else {
        int32_t ea, eb, te;
        uint32_t ma, mb, sa = a >> 31, sb = b >> 31, t, sign;
        uint64_t x, y, m;
        h2s__unpack_norm(a, &ea, &ma);
        h2s__unpack_norm(b, &eb, &mb);
        if (eb > ea) {
            te = ea; ea = eb; eb = te;
            t = ma; ma = mb; mb = t;
            t = sa; sa = sb; sb = t;
        }
        x = (uint64_t)ma << 32;
        y = h2s__shift_right_jam64((uint64_t)mb << 32, (uint32_t)(ea - eb));
        if (sa == sb) { sign = sa; m = x + y; }
        else if (x >= y) { sign = sa; m = x - y; }
        else { sign = sb; m = y - x; }
        if (m == 0) return h2s__mk(h2s__rc_down(mx) ? 0x80000000u : 0, flags);
        r = h2s__norm_round_pack(mx, sign, ea - 150 - 32, m);
    }
    r.f |= flags;
    return r;
}

static inline h2s__res h2s__mul1(uint32_t mx, uint32_t a, uint32_t b) {
    uint32_t sign, flags, ma, mb;
    int32_t ea, eb;
    h2s__res r;
    if (h2s__is_nan(a) || h2s__is_nan(b)) return h2s__nan2(a, b);
    sign = (a ^ b) >> 31;
    if (h2s__is_inf(a) || h2s__is_inf(b)) {
        if (h2s__is_zero(a) || h2s__is_zero(b)) return h2s__mk(H2S_DEFAULT_NAN, H2S_IE);
        return h2s__mk((sign << 31) | 0x7f800000u, h2s__de2(a, b));
    }
    flags = h2s__de2(a, b);
    if (h2s__is_zero(a) || h2s__is_zero(b)) return h2s__mk(sign << 31, flags);
    h2s__unpack_norm(a, &ea, &ma);
    h2s__unpack_norm(b, &eb, &mb);
    r = h2s__norm_round_pack(mx, sign, ea + eb - 300, (uint64_t)ma * mb);
    r.f |= flags;
    return r;
}

static inline h2s__res h2s__div1(uint32_t mx, uint32_t a, uint32_t b) {
    uint32_t sign, flags, ma, mb;
    int32_t ea, eb;
    uint64_t n, q;
    h2s__res r;
    if (h2s__is_nan(a) || h2s__is_nan(b)) return h2s__nan2(a, b);
    sign = (a ^ b) >> 31;
    if (h2s__is_inf(a)) {
        if (h2s__is_inf(b)) return h2s__mk(H2S_DEFAULT_NAN, H2S_IE);
        return h2s__mk((sign << 31) | 0x7f800000u, h2s__de2(a, b));
    }
    if (h2s__is_inf(b)) return h2s__mk(sign << 31, h2s__de2(a, b));
    if (h2s__is_zero(b)) {
        if (h2s__is_zero(a)) return h2s__mk(H2S_DEFAULT_NAN, H2S_IE);
        return h2s__mk((sign << 31) | 0x7f800000u, H2S_ZE);
    }
    flags = h2s__de2(a, b);
    if (h2s__is_zero(a)) return h2s__mk(sign << 31, flags);
    h2s__unpack_norm(a, &ea, &ma);
    h2s__unpack_norm(b, &eb, &mb);
    n = (uint64_t)ma << 39;
    q = n / mb;
    q |= (uint64_t)(q * mb != n);
    r = h2s__norm_round_pack(mx, sign, ea - eb - 39, q);
    r.f |= flags;
    return r;
}

/* floor(sqrt(x)); *rem = x - result^2. */
static inline uint64_t h2s__isqrt64(uint64_t x, uint64_t *rem) {
    uint64_t op = x, res = 0, one = (uint64_t)1 << 62;
    while (one > op) one >>= 2;
    while (one != 0) {
        if (op >= res + one) {
            op -= res + one;
            res = (res >> 1) + one;
        } else {
            res >>= 1;
        }
        one >>= 2;
    }
    *rem = op;
    return res;
}

static inline h2s__res h2s__sqrt1(uint32_t mx, uint32_t b) {
    uint32_t flags, m32;
    int32_t e, k;
    uint64_t m, r, rem;
    h2s__res res;
    if (h2s__is_nan(b)) return h2s__mk(b | H2S__QUIET, h2s__is_snan(b) ? H2S_IE : 0);
    if (h2s__is_zero(b)) return h2s__mk(b, 0);
    if (b >> 31) return h2s__mk(H2S_DEFAULT_NAN, H2S_IE);
    if (h2s__is_inf(b)) return h2s__mk(b, 0);
    flags = h2s__de2(b, 0);
    h2s__unpack_norm(b, &e, &m32);
    k = e - 150;
    m = m32;
    if ((uint32_t)k & 1) { m <<= 1; k -= 1; }
    r = h2s__isqrt64(m << 38, &rem);
    res = h2s__norm_round_pack(mx, 0, (k - 38) / 2 - 1, (r << 1) | (uint64_t)(rem != 0));
    res.f |= flags;
    return res;
}

static inline int h2s__lt(uint32_t a, uint32_t b) {
    uint32_t sa = a >> 31, sb = b >> 31;
    if (h2s__is_zero(a) && h2s__is_zero(b)) return 0;
    if (sa != sb) return sa != 0;
    return sa == 0 ? a < b : a > b;
}
static inline int h2s__eq(uint32_t a, uint32_t b) { return a == b || (h2s__is_zero(a) && h2s__is_zero(b)); }

static inline h2s__res h2s__minmax1(uint32_t a, uint32_t b, int max) {
    int pick_a;
    if (h2s__is_nan(a) || h2s__is_nan(b)) return h2s__mk(b, H2S_IE);
    pick_a = max ? h2s__lt(b, a) : h2s__lt(a, b);
    return h2s__mk(pick_a ? a : b, h2s__de2(a, b));
}

static inline h2s__res h2s__cmp1(uint32_t a, uint32_t b, uint8_t pred_in) {
    unsigned pred = pred_in & 7u;
    int un = h2s__is_nan(a) || h2s__is_nan(b);
    int signalling = pred == 1 || pred == 2 || pred == 5 || pred == 6;
    uint32_t flags;
    int t;
    if (h2s__is_snan(a) || h2s__is_snan(b) || (un && signalling)) flags = H2S_IE;
    else if (un) flags = 0;
    else flags = h2s__de2(a, b);
    switch (pred) {
    case 0: t = !un && h2s__eq(a, b); break;
    case 1: t = !un && h2s__lt(a, b); break;
    case 2: t = !un && (h2s__lt(a, b) || h2s__eq(a, b)); break;
    case 3: t = un; break;
    case 4: t = un || !h2s__eq(a, b); break;
    case 5: t = un || !h2s__lt(a, b); break;
    case 6: t = un || !(h2s__lt(a, b) || h2s__eq(a, b)); break;
    default: t = !un; break;
    }
    return h2s__mk(t ? 0xffffffffu : 0, flags);
}

static inline h2s__res h2s__comi1(uint32_t a, uint32_t b, int signalling) {
    uint32_t r;
    if (h2s__is_nan(a) || h2s__is_nan(b)) {
        int ie = h2s__is_snan(a) || h2s__is_snan(b) || signalling;
        return h2s__mk(0x45, ie ? H2S_IE : 0);
    }
    r = h2s__lt(a, b) ? 0x01u : h2s__eq(a, b) ? 0x40u : 0u;
    return h2s__mk(r, h2s__de2(a, b));
}

static inline h2s__res h2s__i2f1(uint32_t mx, uint32_t i) {
    uint32_t sign, mag;
    if (i == 0) return h2s__mk(0, 0);
    sign = i >> 31;
    mag = sign ? (uint32_t)0 - i : i;
    return h2s__norm_round_pack(mx, sign, 0, mag);
}

static inline h2s__res h2s__f2i1(uint32_t mx, uint32_t a, int truncate) {
    uint32_t exp = (a >> 23) & 0xff, sign = a >> 31, m, frac, rc, r;
    int32_t e, sh;
    uint64_t f, in;
    int up;
    if (exp >= 127 + 32) return h2s__mk(H2S_INT_INDEFINITE, H2S_IE);
    if (exp == 0) { e = 1; m = a & 0x007fffffu; }
    else { e = (int32_t)exp; m = (a & 0x007fffffu) | 0x00800000u; }
    sh = e - 118;
    f = sh >= 0 ? (uint64_t)m << sh : h2s__shift_right_jam64(m, (uint32_t)0 - (uint32_t)sh);
    in = f >> 32;
    frac = (uint32_t)f;
    rc = truncate ? 3 : (mx >> H2S_RC_SHIFT) & 3;
    switch (rc) {
    case 0: up = frac > 0x80000000u || (frac == 0x80000000u && (in & 1)); break;
    case 1: up = sign != 0 && frac != 0; break;
    case 2: up = sign == 0 && frac != 0; break;
    default: up = 0; break;
    }
    in += (uint64_t)up;
    if (in > 0x7fffffffull + sign) return h2s__mk(H2S_INT_INDEFINITE, H2S_IE);
    r = sign ? (uint32_t)0 - (uint32_t)in : (uint32_t)in;
    return h2s__mk(r, frac != 0 ? H2S_PE : 0);
}

/* The Pentium III's rcp approximation: 1/midpoint of the interval the top 11
 * fraction bits select, rounded to nearest at 12 fraction bits. See
 * rcp_approx in src/lib.rs and docs/lifter/simd.md. */
static inline uint32_t h2s_rcp_approx(uint32_t x) {
    uint32_t s = x & 0x80000000u, e = (x >> 23) & 0xff, f = x & 0x007fffffu, d, n;
    int32_t oe;
    if (e == 0xff) return f != 0 ? (x | H2S__QUIET) : s;
    if (e == 0) return s | 0x7f800000u;
    d = 4097 + 2 * (f >> 12);
    n = ((1u << 26) + d) / (2 * d);
    oe = 253 - (int32_t)e;
    if (oe <= 0) return s;
    return s | ((uint32_t)oe << 23) | ((n - 4096) << 11);
}

/* The Pentium III's rsqrt approximation: 1/sqrt(midpoint) of the interval of
 * [1, 4) that the exponent parity and top 10 fraction bits select, rounded
 * to nearest at 12 fraction bits. See rsqrt_approx in src/lib.rs. */
static inline uint32_t h2s_rsqrt_approx(uint32_t x) {
    uint32_t s = x & 0x80000000u, e = (x >> 23) & 0xff, f = x & 0x007fffffu, p, d, n;
    uint64_t m, rem;
    int32_t oe;
    if (e == 0xff) return f != 0 ? (x | H2S__QUIET) : s ? H2S_DEFAULT_NAN : 0;
    if (e == 0) return s | 0x7f800000u;
    if (s) return H2S_DEFAULT_NAN;
    p = (e & 1) ^ 1;
    d = 2049 + 2 * (f >> 13);
    m = h2s__isqrt64(((uint64_t)1 << (39 - p)) / d, &rem);
    n = (uint32_t)((m + 1) >> 1);
    oe = 126 - ((int32_t)e - 127 - (int32_t)p) / 2;
    return ((uint32_t)oe << 23) | ((n - 4096) << 11);
}

/* ---- lanes and exception delivery ---- */
static inline uint32_t h2s__lane(h2u128 v, unsigned i) { return (uint32_t)(v >> (32 * i)); }
static inline h2u128 h2s__with_lane0(h2u128 a, uint32_t x) { return (a & ~(h2u128)0xffffffffu) | x; }

/* Records flags in MXCSR; returns 1 (#XM) on an unmasked exception. An
 * unmasked pre-computation exception (IE, DE, ZE) records only the
 * pre-computation flags. */
static inline uint32_t h2s__deliver(uint32_t *mxcsr, uint32_t flags) {
    uint32_t unmasked = ~(*mxcsr >> H2S_MASK_SHIFT) & 0x3f;
    if (flags & H2S__PRE & unmasked) {
        *mxcsr |= flags & H2S__PRE;
        return 1;
    }
    *mxcsr |= flags;
    return (flags & unmasked) != 0;
}

static inline uint32_t h2s__scalar(uint32_t *mxcsr, h2u128 a, h2s__res r, h2u128 *out) {
    if (h2s__deliver(mxcsr, r.f)) return 1;
    *out = h2s__with_lane0(a, r.r);
    return 0;
}

#define H2S__PACKED(EXPR)                                                      \
    do {                                                                       \
        uint32_t flags_ = 0;                                                   \
        h2u128 acc_ = 0;                                                       \
        unsigned i_;                                                           \
        for (i_ = 0; i_ < 4; i_++) {                                           \
            uint32_t x = h2s__lane(a, i_), y = h2s__lane(b, i_);               \
            h2s__res r_ = (EXPR);                                              \
            (void)x;                                                           \
            (void)y;                                                           \
            flags_ |= r_.f;                                                    \
            acc_ |= (h2u128)r_.r << (32 * i_);                                 \
        }                                                                      \
        if (h2s__deliver(mxcsr, flags_)) return 1;                             \
        *out = acc_;                                                           \
        return 0;                                                              \
    } while (0)

/* ---- the instructions ---- */
#define H2S__ARITH(ss, ps, EXPR)                                                          \
    static inline uint32_t h2s_##ss(uint32_t *mxcsr, h2u128 a, uint32_t b, h2u128 *out) { \
        uint32_t m = *mxcsr, x = h2s__lane(a, 0), y = b;                                  \
        (void)m;                                                                          \
        return h2s__scalar(mxcsr, a, (EXPR), out);                                        \
    }                                                                                     \
    static inline uint32_t h2s_##ps(uint32_t *mxcsr, h2u128 a, h2u128 b, h2u128 *out) {   \
        uint32_t m = *mxcsr;                                                              \
        (void)m;                                                                          \
        H2S__PACKED(EXPR);                                                                \
    }

H2S__ARITH(addss, addps, h2s__add1(m, x, y, 0))
H2S__ARITH(subss, subps, h2s__add1(m, x, y, 1))
H2S__ARITH(mulss, mulps, h2s__mul1(m, x, y))
H2S__ARITH(divss, divps, h2s__div1(m, x, y))
H2S__ARITH(minss, minps, h2s__minmax1(x, y, 0))
H2S__ARITH(maxss, maxps, h2s__minmax1(x, y, 1))

static inline uint32_t h2s_sqrtss(uint32_t *mxcsr, h2u128 a, uint32_t b, h2u128 *out) {
    return h2s__scalar(mxcsr, a, h2s__sqrt1(*mxcsr, b), out);
}
static inline uint32_t h2s_sqrtps(uint32_t *mxcsr, h2u128 b, h2u128 *out) {
    uint32_t m = *mxcsr;
    h2u128 a = b;
    H2S__PACKED(h2s__sqrt1(m, y));
}

static inline uint32_t h2s_cmpss(uint32_t *mxcsr, h2u128 a, uint32_t b, uint8_t pred, h2u128 *out) {
    return h2s__scalar(mxcsr, a, h2s__cmp1(h2s__lane(a, 0), b, pred), out);
}
static inline uint32_t h2s_cmpps(uint32_t *mxcsr, h2u128 a, h2u128 b, uint8_t pred, h2u128 *out) {
    H2S__PACKED(h2s__cmp1(x, y, pred));
}

static inline uint32_t h2s__eflags(uint32_t *mxcsr, h2s__res r, uint32_t *out) {
    if (h2s__deliver(mxcsr, r.f)) return 1;
    *out = r.r;
    return 0;
}
/* EFLAGS ZF (0x40), PF (0x04), CF (0x01): unordered 0x45, less 0x01, equal
 * 0x40, greater 0. The caller clears OF, SF and AF. */
static inline uint32_t h2s_comiss(uint32_t *mxcsr, uint32_t a, uint32_t b, uint32_t *out) {
    return h2s__eflags(mxcsr, h2s__comi1(a, b, 1), out);
}
static inline uint32_t h2s_ucomiss(uint32_t *mxcsr, uint32_t a, uint32_t b, uint32_t *out) {
    return h2s__eflags(mxcsr, h2s__comi1(a, b, 0), out);
}

static inline uint32_t h2s_cvtsi2ss(uint32_t *mxcsr, h2u128 a, uint32_t i, h2u128 *out) {
    return h2s__scalar(mxcsr, a, h2s__i2f1(*mxcsr, i), out);
}
static inline uint32_t h2s_cvtpi2ps(uint32_t *mxcsr, h2u128 a, uint64_t mm, h2u128 *out) {
    h2s__res r0 = h2s__i2f1(*mxcsr, (uint32_t)mm), r1 = h2s__i2f1(*mxcsr, (uint32_t)(mm >> 32));
    if (h2s__deliver(mxcsr, r0.f | r1.f)) return 1;
    *out = (a & ~(h2u128)0xffffffffffffffffull) | r0.r | (h2u128)r1.r << 32;
    return 0;
}
static inline uint32_t h2s_cvtss2si(uint32_t *mxcsr, uint32_t a, uint32_t *out) {
    return h2s__eflags(mxcsr, h2s__f2i1(*mxcsr, a, 0), out);
}
static inline uint32_t h2s_cvttss2si(uint32_t *mxcsr, uint32_t a, uint32_t *out) {
    return h2s__eflags(mxcsr, h2s__f2i1(*mxcsr, a, 1), out);
}
static inline uint32_t h2s__f2i2(uint32_t *mxcsr, uint64_t a, int truncate, uint64_t *out) {
    h2s__res r0 = h2s__f2i1(*mxcsr, (uint32_t)a, truncate), r1 = h2s__f2i1(*mxcsr, (uint32_t)(a >> 32), truncate);
    if (h2s__deliver(mxcsr, r0.f | r1.f)) return 1;
    *out = r0.r | (uint64_t)r1.r << 32;
    return 0;
}
static inline uint32_t h2s_cvtps2pi(uint32_t *mxcsr, uint64_t a, uint64_t *out) { return h2s__f2i2(mxcsr, a, 0, out); }
static inline uint32_t h2s_cvttps2pi(uint32_t *mxcsr, uint64_t a, uint64_t *out) { return h2s__f2i2(mxcsr, a, 1, out); }

static inline h2u128 h2s_rcpss(h2u128 a, uint32_t b) { return h2s__with_lane0(a, h2s_rcp_approx(b)); }
static inline h2u128 h2s_rsqrtss(h2u128 a, uint32_t b) { return h2s__with_lane0(a, h2s_rsqrt_approx(b)); }
static inline h2u128 h2s_rcpps(h2u128 b) {
    h2u128 r = 0;
    unsigned i;
    for (i = 0; i < 4; i++) r |= (h2u128)h2s_rcp_approx(h2s__lane(b, i)) << (32 * i);
    return r;
}
static inline h2u128 h2s_rsqrtps(h2u128 b) {
    h2u128 r = 0;
    unsigned i;
    for (i = 0; i < 4; i++) r |= (h2u128)h2s_rsqrt_approx(h2s__lane(b, i)) << (32 * i);
    return r;
}

#endif
