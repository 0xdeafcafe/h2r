/* Randomized equivalence of the inlined native SSE fast paths with the
 * integer authority (h2simd.h). Every accepted operation must give the
 * same destination bits and MXCSR as h2s_*; the NEON packed path must
 * accept exactly when the scalar lane path does, with the same result.
 * Usage: sse_fast_equiv COUNT SEED. Prints acceptance counts per op. */
#include <stdio.h>
#include <stdlib.h>
#include "h2sse_native.h"

static uint64_t rng_state;
static uint64_t next(void) {
    uint64_t x = rng_state;
    x ^= x << 13; x ^= x >> 7; x ^= x << 17;
    return rng_state = x;
}
static uint32_t below(uint32_t n) { return (uint32_t)(next() % n); }

/* Lanes: mostly the native range, plus its edges and every kind of
 * special the guard must reject. */
static uint32_t lane(void) {
    uint32_t sign = (uint32_t)next() & 0x80000000u, frac = (uint32_t)next() & 0x7fffffu;
    switch (below(16)) {
    case 0: return sign; /* signed zero */
    case 1: return sign | ((66u + below(3)) << 23) | frac; /* low edge 66,67,68 */
    case 2: return sign | ((186u + below(3)) << 23) | frac; /* high edge 186..188 */
    case 3: return sign | 0x7f800000u; /* infinity */
    case 4: return sign | 0x7f800000u | (frac ? frac : 1u); /* NaN */
    case 5: return sign | (frac ? frac : 1u); /* denormal */
    case 6: return (uint32_t)next(); /* raw */
    case 7: return sign | ((1u + below(254)) << 23) | frac; /* any normal */
    default: return sign | ((67u + below(121)) << 23) | frac; /* native range */
    }
}
static h2u128 vec(void) {
    h2u128 v = 0;
    for (unsigned i = 0; i < 4; ++i) v |= (h2u128)lane() << (32u * i);
    return v;
}
/* Correlated operands: cancellation and exact results. */
static h2u128 near(h2u128 a) {
    h2u128 v = 0;
    for (unsigned i = 0; i < 4; ++i) {
        uint32_t x = (uint32_t)(a >> (32u * i));
        switch (below(4)) {
        case 0: x ^= 1u << below(23); break;
        case 1: x ^= 0x80000000u; break; /* exact cancellation in add */
        case 2: x = (x & 0xff800000u) | (x & 0x7fffffu & ~((1u << below(24)) - 1u)); break;
        default: x = lane(); break;
        }
        v |= (h2u128)x << (32u * i);
    }
    return v;
}

static uint32_t authority(unsigned op, int packed, uint32_t *mx, h2u128 a, h2u128 b, h2u128 *r) {
    uint32_t b32 = (uint32_t)b;
    switch (op * 2 + (unsigned)packed) {
    case 0: return h2s_addss(mx, a, b32, r);
    case 1: return h2s_addps(mx, a, b, r);
    case 2: return h2s_subss(mx, a, b32, r);
    case 3: return h2s_subps(mx, a, b, r);
    case 4: return h2s_mulss(mx, a, b32, r);
    case 5: return h2s_mulps(mx, a, b, r);
    case 6: return h2s_divss(mx, a, b32, r);
    case 7: return h2s_divps(mx, a, b, r);
    case 8: return h2s_sqrtss(mx, a, b32, r);
    default: return h2s_sqrtps(mx, b, r);
    }
}
/* The public entry points used by generated code. */
static uint32_t entry(unsigned op, int packed, uint32_t *mx, h2u128 a, h2u128 b, h2u128 *r) {
    uint32_t b32 = (uint32_t)b;
    switch (op * 2 + (unsigned)packed) {
    case 0: return h2sn_addss(mx, a, b32, r);
    case 1: return h2sn_addps(mx, a, b, r);
    case 2: return h2sn_subss(mx, a, b32, r);
    case 3: return h2sn_subps(mx, a, b, r);
    case 4: return h2sn_mulss(mx, a, b32, r);
    case 5: return h2sn_mulps(mx, a, b, r);
    case 6: return h2sn_divss(mx, a, b32, r);
    case 7: return h2sn_divps(mx, a, b, r);
    case 8: return h2sn_sqrtss(mx, a, b32, r);
    default: return h2sn_sqrtps(mx, b, r);
    }
}

static int fail(const char *what, unsigned op, int packed, uint32_t mx, h2u128 a, h2u128 b) {
    fprintf(stderr, "%s: op %u packed %d mxcsr %08x a %016llx%016llx b %016llx%016llx\n", what, op,
            packed, mx, (unsigned long long)(a >> 64), (unsigned long long)a,
            (unsigned long long)(b >> 64), (unsigned long long)b);
    return 1;
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    unsigned long count = strtoul(argv[1], 0, 10);
    rng_state = strtoull(argv[2], 0, 0) | 1u;
    unsigned long accepted[5][2] = {{0}}, total[5][2] = {{0}};
    for (unsigned long n = 0; n < count; ++n) {
        unsigned op = below(5);
        int packed = (int)below(2);
        uint32_t mx = 0x1f80u | ((uint32_t)next() & 0x803fu); /* sticky flags, FTZ */
        if (below(8) == 0) mx = (uint32_t)next() & 0xffffu; /* any mode */
        h2u128 a = vec(), b = below(3) == 0 ? near(a) : vec();
        if (below(4) == 0) { /* all four lanes in range: the common case */
            h2u128 v = 0, w = 0;
            for (unsigned i = 0; i < 4; ++i) {
                uint32_t f = (uint32_t)next() & 0x807fffffu;
                v |= (h2u128)(f | ((67u + below(121)) << 23)) << (32u * i);
                f = (uint32_t)next() & (op == 4 ? 0x7fffffu : 0x807fffffu);
                w |= (h2u128)(f | ((67u + below(121)) << 23)) << (32u * i);
            }
            a = v;
            b = below(2) ? w : near(v);
        }
        total[op][packed]++;
        uint32_t m_ref = mx, m_fast = mx, m_scalar = mx;
        h2u128 r_ref = 0, r_fast = 0, r_scalar = 0;
        uint32_t xm = authority(op, packed, &m_ref, a, b, &r_ref);
        int took;
        (void)m_scalar; (void)r_scalar;
#if defined(__aarch64__) || (defined(__x86_64__) && H2_SSE_PACKED_X86)
        if (packed) {
            took = h2sn_arith4(&m_fast, op, op == 4 ? 0 : a, b, &r_fast);
            int took_scalar = h2sn_arith(&m_scalar, op, op == 4 ? 0 : a, b, 4, &r_scalar);
            if (took != took_scalar) return fail("packed and scalar guards disagree", op, packed, mx, a, b);
            if (took && (r_fast != r_scalar || m_fast != m_scalar))
                return fail("packed and scalar results differ", op, packed, mx, a, b);
        } else
#endif
            took = h2sn_arith(&m_fast, op, a, b, packed ? 4 : 1, &r_fast);
        if (took) {
            accepted[op][packed]++;
            if (xm || r_fast != r_ref || m_fast != m_ref)
                return fail("fast path differs from the authority", op, packed, mx, a, b);
        }
        /* The public entry point, fast or slow, always equals the authority. */
        uint32_t m_entry = mx;
        h2u128 r_entry = 0;
        uint32_t xe = entry(op, packed, &m_entry, a, b, &r_entry);
        if (xe != xm || m_entry != m_ref || (!xm && r_entry != r_ref))
            return fail("entry point differs from the authority", op, packed, mx, a, b);
    }
    static const char *names[5] = {"add", "sub", "mul", "div", "sqrt"};
    for (unsigned op = 0; op < 5; ++op)
        for (int p = 0; p < 2; ++p)
            printf("%s%s %lu %lu\n", names[op], p ? "ps" : "ss", accepted[op][p], total[op][p]);
    return 0;
}
