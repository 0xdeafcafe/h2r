/* Guarded binary32 arithmetic. See docs/sse-native.md for the range and
 * precision proof. The integer authority handles every rejected case.
 *
 * No FENV_ACCESS: strict-FP functions cannot be inlined into the generated
 * (non-strict) handlers, which made every packed op several real calls.
 * Instead every FP operation consumes operands laundered through an empty
 * volatile asm placed after the guards, so none can be evaluated before
 * the guest/host mode and operand checks have passed. Past the guard the
 * host is round-to-nearest with exceptions masked, which is exactly the
 * environment the non-strict compiler assumes. */
#ifndef H2SSE_NATIVE_H
#define H2SSE_NATIVE_H
#include "h2simd.h"
#include <string.h>
#include <math.h>
#if H2_SSE_NATIVE
#if defined(__aarch64__)
#include <arm_neon.h>
#endif
static inline __attribute__((always_inline)) int h2sn_host_mode(void) {
#if defined(__aarch64__)
    uint64_t fpcr;
    __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
    return !(fpcr & UINT64_C(0xc09f00));
#elif defined(__x86_64__)
    uint32_t mxcsr;
    __asm__ volatile("stmxcsr %0" : "=m"(mxcsr));
    return (mxcsr & 0x7f80u) == 0x1f80u;
#else
    return 0;
#endif
}
static inline __attribute__((always_inline)) int h2sn_value(uint32_t b) {
    unsigned e = (b >> 23) & 255u;
    return !(b & 0x7fffffffu) || (e >= 67u && e <= 187u);
}
/* The speculation barrier: the value is opaque to the compiler, and the
 * volatile asm cannot be hoisted above the branch that guards it. */
static inline __attribute__((always_inline)) float h2sn_float(uint32_t b) {
    __asm__ volatile("" : "+r"(b));
    float x; memcpy(&x, &b, 4); return x;
}
static inline __attribute__((always_inline)) int h2sn_mode(uint32_t mx) {
    return (mx & 0x7f80u) == 0x1f80u && h2sn_host_mode();
}
static inline __attribute__((always_inline)) int h2sn_ok(unsigned op, uint32_t a, uint32_t b) {
    return h2sn_value(b) && (op == 4u || h2sn_value(a)) &&
           !(op == 3u && !(b & 0x7fffffffu)) &&
           !(op == 4u && (b >> 31) && (b & 0x7fffffffu));
}
/* op: add, sub, mul, div, sqrt. Call only once h2sn_ok has accepted every
 * lane: a failed lane leaves the whole instruction to the original helper. */
static inline __attribute__((always_inline)) uint32_t h2sn_lane(unsigned op, uint32_t a, uint32_t b, int *inexact) {
    float x = op == 4u ? 0.0f : h2sn_float(a), y = h2sn_float(b), r;
    if (op <= 1u) {
        double u = (double)x, v = op ? -(double)y : (double)y;
        r = op ? x - y : x + y;
        /* FastTwoSum: exact sum is s + e, including very unequal inputs. */
        if (fabs(u) < fabs(v)) { double t = u; u = v; v = t; }
        double s = u + v, e = v - (s - u);
        *inexact |= (double)r != s || e != 0.0;
    } else if (op == 2u) {
        r = x * y;
        *inexact |= (double)r != (double)x * (double)y;
    } else if (op == 3u) {
        r = x / y;
        *inexact |= (double)r * (double)y != (double)x;
    } else {
        r = sqrtf(y);
        *inexact |= (double)r * (double)r != (double)y;
    }
    uint32_t out;
    memcpy(&out, &r, 4);
    return out;
}
static inline __attribute__((always_inline)) int h2sn_arith(uint32_t *mx, unsigned op, h2u128 a, h2u128 b,
                           unsigned lanes, h2u128 *out) {
    if (!h2sn_mode(*mx)) return 0;
    for (unsigned i = 0; i < lanes; ++i)
        if (!h2sn_ok(op, (uint32_t)(a >> (32u * i)), (uint32_t)(b >> (32u * i)))) return 0;
    h2u128 result = lanes == 1u ? a & ~((h2u128)UINT32_MAX) : 0;
    int inexact = 0;
    for (unsigned i = 0; i < lanes; ++i)
        result |= (h2u128)h2sn_lane(op, (uint32_t)(a >> (32u * i)),
                                    (uint32_t)(b >> (32u * i)), &inexact) << (32u * i);
    *mx |= inexact ? H2S_PE : 0u;
    *out = result;
    return 1;
}
#if defined(__aarch64__)
/* All four lanes at once; the same guards and PE reconstruction as
 * h2sn_lane, lane for lane (docs/sse-native.md). */
static inline __attribute__((always_inline)) uint32x4_t h2sn_value4(uint32x4_t v) {
    uint32x4_t mag = vandq_u32(v, vdupq_n_u32(0x7fffffffu));
    uint32x4_t e = vandq_u32(vshrq_n_u32(v, 23), vdupq_n_u32(255u));
    uint32x4_t in = vcleq_u32(vsubq_u32(e, vdupq_n_u32(67u)), vdupq_n_u32(120u));
    return vorrq_u32(vceqzq_u32(mag), in);
}
static inline __attribute__((always_inline)) uint64x2_t h2sn_two_sum4(float64x2_t u, float64x2_t v, float64x2_t r) {
    uint64x2_t keep = vcageq_f64(u, v); /* |u| >= |v|: no swap */
    float64x2_t big = vbslq_f64(keep, u, v), small = vbslq_f64(keep, v, u);
    float64x2_t s = vaddq_f64(big, small);
    float64x2_t e = vsubq_f64(small, vsubq_f64(s, big));
    uint64x2_t exact = vandq_u64(vceqq_f64(r, s), vceqzq_f64(e));
    return vreinterpretq_u64_u32(vmvnq_u32(vreinterpretq_u32_u64(exact)));
}
static inline __attribute__((always_inline)) int h2sn_arith4(uint32_t *mx, unsigned op, h2u128 a, h2u128 b, h2u128 *out) {
    if (!h2sn_mode(*mx)) return 0;
    uint32x4_t va, vb;
    memcpy(&va, &a, 16);
    memcpy(&vb, &b, 16);
    uint32x4_t ok = h2sn_value4(vb);
    uint32x4_t bmag = vandq_u32(vb, vdupq_n_u32(0x7fffffffu));
    if (op != 4u) ok = vandq_u32(ok, h2sn_value4(va));
    if (op == 3u) ok = vbicq_u32(ok, vceqzq_u32(bmag));
    if (op == 4u) ok = vbicq_u32(ok, vtstq_u32(vb, vdupq_n_u32(0x80000000u)) & vtstq_u32(bmag, bmag));
    if (vminvq_u32(ok) == 0) return 0;
    __asm__ volatile("" : "+w"(va), "+w"(vb));
    float32x4_t x = vreinterpretq_f32_u32(va), y = vreinterpretq_f32_u32(vb), r;
    float64x2_t xl = vcvt_f64_f32(vget_low_f32(x)), xh = vcvt_high_f64_f32(x);
    float64x2_t yl = vcvt_f64_f32(vget_low_f32(y)), yh = vcvt_high_f64_f32(y);
    uint64x2_t pl, ph; /* lanes whose rounding was inexact */
    if (op <= 1u) {
        r = op ? vsubq_f32(x, y) : vaddq_f32(x, y);
        if (op) { yl = vnegq_f64(yl); yh = vnegq_f64(yh); }
        pl = h2sn_two_sum4(xl, yl, vcvt_f64_f32(vget_low_f32(r)));
        ph = h2sn_two_sum4(xh, yh, vcvt_high_f64_f32(r));
    } else {
        float64x2_t rl, rh;
        if (op == 2u) {
            r = vmulq_f32(x, y);
        } else if (op == 3u) {
            r = vdivq_f32(x, y);
        } else {
            r = vsqrtq_f32(y);
        }
        rl = vcvt_f64_f32(vget_low_f32(r));
        rh = vcvt_high_f64_f32(r);
        /* All products below are of two binary32 values: exact in binary64. */
        if (op == 2u) {
            pl = vceqq_f64(rl, vmulq_f64(xl, yl));
            ph = vceqq_f64(rh, vmulq_f64(xh, yh));
        } else if (op == 3u) {
            pl = vceqq_f64(vmulq_f64(rl, yl), xl);
            ph = vceqq_f64(vmulq_f64(rh, yh), xh);
        } else {
            pl = vceqq_f64(vmulq_f64(rl, rl), yl);
            ph = vceqq_f64(vmulq_f64(rh, rh), yh);
        }
        pl = vreinterpretq_u64_u32(vmvnq_u32(vreinterpretq_u32_u64(pl)));
        ph = vreinterpretq_u64_u32(vmvnq_u32(vreinterpretq_u32_u64(ph)));
    }
    uint64x2_t any = vorrq_u64(pl, ph);
    *mx |= (vgetq_lane_u64(any, 0) | vgetq_lane_u64(any, 1)) ? H2S_PE : 0u;
    memcpy(out, &r, 16);
    return 1;
}
#define H2SN_TRY4(op, a, b) \
    if (h2sn_arith4(mx, op, a, b, out)) return 0
#else
#define H2SN_TRY4(op, a, b) \
    if (h2sn_arith(mx, op, a, b, 4, out)) return 0
#endif
#define H2SN_TRY(op, a, b, lanes) \
    if (h2sn_arith(mx, op, a, b, lanes, out)) return 0
#else
#define H2SN_TRY(op, a, b, lanes) ((void)0)
#define H2SN_TRY4(op, a, b) ((void)0)
#endif
/* The guarded native path inlines into the handler; the integer authority
 * stays out of line so each generated op adds only the fast path. */
#define H2SN_SLOW __attribute__((noinline, cold))
#define H2SN_FAST __attribute__((always_inline))
#define H2SN_BINARY(name, op) \
    static H2SN_SLOW uint32_t h2sn_##name##ss_slow(uint32_t *mx, h2u128 a, uint32_t b, h2u128 *out) { \
        return h2s_##name##ss(mx, a, b, out); \
    } \
    static H2SN_SLOW uint32_t h2sn_##name##ps_slow(uint32_t *mx, h2u128 a, h2u128 b, h2u128 *out) { \
        return h2s_##name##ps(mx, a, b, out); \
    } \
    static inline H2SN_FAST uint32_t h2sn_##name##ss(uint32_t *mx, h2u128 a, uint32_t b, h2u128 *out) { \
        H2SN_TRY(op, a, b, 1); return h2sn_##name##ss_slow(mx, a, b, out); \
    } \
    static inline H2SN_FAST uint32_t h2sn_##name##ps(uint32_t *mx, h2u128 a, h2u128 b, h2u128 *out) { \
        H2SN_TRY4(op, a, b); return h2sn_##name##ps_slow(mx, a, b, out); \
    }
H2SN_BINARY(add, 0u) H2SN_BINARY(sub, 1u)
H2SN_BINARY(mul, 2u) H2SN_BINARY(div, 3u)
static H2SN_SLOW uint32_t h2sn_sqrtss_slow(uint32_t *mx, h2u128 a, uint32_t b, h2u128 *out) {
    return h2s_sqrtss(mx, a, b, out);
}
static H2SN_SLOW uint32_t h2sn_sqrtps_slow(uint32_t *mx, h2u128 b, h2u128 *out) {
    return h2s_sqrtps(mx, b, out);
}
static inline H2SN_FAST uint32_t h2sn_sqrtss(uint32_t *mx, h2u128 a, uint32_t b, h2u128 *out) {
    H2SN_TRY(4u, a, b, 1); return h2sn_sqrtss_slow(mx, a, b, out);
}
static inline H2SN_FAST uint32_t h2sn_sqrtps(uint32_t *mx, h2u128 b, h2u128 *out) {
    H2SN_TRY4(4u, 0, b); return h2sn_sqrtps_slow(mx, b, out);
}
#undef H2SN_TRY
#undef H2SN_TRY4
#undef H2SN_BINARY
#undef H2SN_SLOW
#undef H2SN_FAST
#endif
