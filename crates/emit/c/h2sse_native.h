/* Guarded binary32 arithmetic. See docs/sse-native.md for the range and
 * precision proof. The integer authority handles every rejected case. */
#ifndef H2SSE_NATIVE_H
#define H2SSE_NATIVE_H
#include "h2simd.h"
#include <string.h>
#include <math.h>
#if H2_SSE_NATIVE
#pragma STDC FENV_ACCESS ON
static inline int h2sn_host_mode(void) {
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
static inline int h2sn_value(uint32_t b) {
    unsigned e = (b >> 23) & 255u;
    return !(b & 0x7fffffffu) || (e >= 67u && e <= 187u);
}
static inline float h2sn_float(uint32_t b) {
    float x; memcpy(&x, &b, 4); return x;
}
static inline int h2sn_mode(uint32_t mx) {
    return (mx & 0x7f80u) == 0x1f80u && h2sn_host_mode();
}
/* op: add, sub, mul, div, sqrt. No FP before every eligibility check.
 * A failed lane leaves the whole instruction to the original helper. */
static inline int h2sn_lane(unsigned op, uint32_t a, uint32_t b,
                           uint32_t *out, uint32_t *flags) {
    if (!h2sn_value(b) || (op != 4u && !h2sn_value(a)) ||
        (op == 3u && !(b & 0x7fffffffu)) ||
        (op == 4u && (b >> 31) && (b & 0x7fffffffu))) return 0;
    float x = op == 4u ? 0.0f : h2sn_float(a), y = h2sn_float(b), r;
    int inexact;
    if (op <= 1u) {
        double u = (double)x, v = op ? -(double)y : (double)y;
        r = op ? x - y : x + y;
        /* FastTwoSum: exact sum is s + e, including very unequal inputs. */
        if (fabs(u) < fabs(v)) { double t = u; u = v; v = t; }
        double s = u + v, e = v - (s - u);
        inexact = (double)r != s || e != 0.0;
    } else if (op == 2u) {
        r = x * y;
        inexact = (double)r != (double)x * (double)y;
    } else if (op == 3u) {
        r = x / y;
        inexact = (double)r * (double)y != (double)x;
    } else {
        r = sqrtf(y);
        inexact = (double)r * (double)r != (double)y;
    }
    memcpy(out, &r, 4);
    *flags |= inexact ? H2S_PE : 0u;
    return 1;
}
static inline int h2sn_arith(uint32_t *mx, unsigned op, h2u128 a, h2u128 b,
                           unsigned lanes, h2u128 *out) {
    if (!h2sn_mode(*mx)) return 0;
    h2u128 result = lanes == 1u ? a & ~((h2u128)UINT32_MAX) : 0;
    uint32_t flags = 0;
    for (unsigned i = 0; i < lanes; ++i) {
        uint32_t r;
        if (!h2sn_lane(op, (uint32_t)(a >> (32u * i)),
                       (uint32_t)(b >> (32u * i)), &r, &flags)) return 0;
        result |= (h2u128)r << (32u * i);
    }
    *mx |= flags;
    *out = result;
    return 1;
}
#define H2SN_TRY(op, a, b, lanes) \
    if (h2sn_arith(mx, op, a, b, lanes, out)) return 0
#else
#define H2SN_TRY(op, a, b, lanes) ((void)0)
#endif
#define H2SN_BINARY(name, op) \
    static inline uint32_t h2sn_##name##ss(uint32_t *mx, h2u128 a, uint32_t b, h2u128 *out) { \
        H2SN_TRY(op, a, b, 1); return h2s_##name##ss(mx, a, b, out); \
    } \
    static inline uint32_t h2sn_##name##ps(uint32_t *mx, h2u128 a, h2u128 b, h2u128 *out) { \
        H2SN_TRY(op, a, b, 4); return h2s_##name##ps(mx, a, b, out); \
    }
H2SN_BINARY(add, 0u) H2SN_BINARY(sub, 1u)
H2SN_BINARY(mul, 2u) H2SN_BINARY(div, 3u)
static inline uint32_t h2sn_sqrtss(uint32_t *mx, h2u128 a, uint32_t b, h2u128 *out) {
    H2SN_TRY(4u, a, b, 1); return h2s_sqrtss(mx, a, b, out);
}
static inline uint32_t h2sn_sqrtps(uint32_t *mx, h2u128 b, h2u128 *out) {
    H2SN_TRY(4u, 0, b, 4); return h2s_sqrtps(mx, b, out);
}
#undef H2SN_TRY
#undef H2SN_BINARY
#endif
