/* Per-op timing of the guarded native x87 operations (h2x87_native.h) as
 * generated handlers call them, at PC=53/nearest/masked with operands in
 * the native window. The authority is stubbed: reaching it aborts, so
 * every timed op is the fast path. Prints ns per op (minimum of 15). */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "h2x87.h"
#define STUB(name, ...) uint32_t name(__VA_ARGS__) { abort(); }
STUB(h2x87_v1_arith, X87Abi *s, uint32_t op, uint32_t dst, uint32_t kind, uint64_t src)
STUB(h2x87_v1_unary, X87Abi *s, uint32_t op)
STUB(h2x87_v1_ld_f64, X87Abi *s, uint64_t b)
STUB(h2x87_v1_ld_f32, X87Abi *s, uint32_t b)
STUB(h2x87_v1_ld_const, X87Abi *s, uint32_t w)
STUB(h2x87_v1_ld_st, X87Abi *s, uint32_t i)
STUB(h2x87_v1_st_st, X87Abi *s, uint32_t i, uint32_t pop)
STUB(h2x87_v1_xch, X87Abi *s, uint32_t i)
STUB(h2x87_v1_st_f64, X87Abi *s, uint32_t i, uint64_t *v, uint16_t *sw)
STUB(h2x87_v1_st_f32, X87Abi *s, uint32_t i, uint32_t *v, uint16_t *sw)
#include "h2x87_native.h"

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec * 1e9 + (double)t.tv_nsec;
}
#define ITERS 2000000
/* Load a binary32 from "memory", operate with a memory or register
 * source, store binary32: the shape of most game x87 code. */
#define LOOP(name, body)                                                    \
    __attribute__((noinline)) static uint32_t loop_##name(X87Abi *s, const uint32_t *m, uint32_t *out) { \
        uint32_t st = 0; uint16_t sw;                                       \
        for (int i = 0; i < ITERS; ++i) {                                   \
            st |= h2x87n_ld_f32(s, m[i & 7]);                               \
            st |= body;                                                     \
            st |= h2x87n_st_f32(s, 0, &out[i & 7], &sw);                    \
            st |= h2x87n_store_commit(s, sw, 1);                            \
        }                                                                   \
        return st;                                                          \
    }
LOOP(fadd_m32, h2x87n_arith(s, 0u, 0u, 1u, m[(i + 3) & 7]))
LOOP(fmul_m32, h2x87n_arith(s, 3u, 0u, 1u, m[(i + 3) & 7]))
LOOP(fdiv_m32, h2x87n_arith(s, 4u, 0u, 1u, m[(i + 3) & 7]))
LOOP(fsqrt, h2x87n_unary(s, 0u))
LOOP(none, 0u)

int main(void) {
    static X87Abi s;
    static const float vals[8] = {1.25f, 3.5f, 0.75f, 2.0f, 1.0001f, 0.9999f, 7.0f, 1.5f};
    uint32_t m[8], out[8];
    memcpy(m, vals, sizeof m);
    struct { const char *name; uint32_t (*f)(X87Abi *, const uint32_t *, uint32_t *); } ops[] = {
        {"fld+fadd_m32+fstp", loop_fadd_m32}, {"fld+fmul_m32+fstp", loop_fmul_m32},
        {"fld+fdiv_m32+fstp", loop_fdiv_m32}, {"fld+fsqrt+fstp", loop_fsqrt},
        {"fld+fstp", loop_none},
    };
    for (unsigned k = 0; k < sizeof ops / sizeof *ops; ++k) {
        double best = 1e30;
        uint32_t sink = 0;
        for (int run = 0; run < 15; ++run) {
            memset(&s, 0, sizeof s);
            s.control = 0x027f; s.tag = 0xffff;
            double t = now();
            sink |= ops[k].f(&s, m, out);
            t = now() - t;
            if (t < best) best = t;
        }
        printf("X87_BENCH %s ns_per_seq=%.2f status=%u\n", ops[k].name, best / ITERS, sink);
    }
    return 0;
}
