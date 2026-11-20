/* Per-op timing of the native SSE entry points as generated handlers use
 * them: XMM registers and MXCSR in a CPU struct, each op loading its
 * operands and storing its result. Operands stay in the native range so
 * every op takes the fast path. Prints ns per op (minimum of 15 runs). */
#include <stdio.h>
#include <time.h>
#include "h2sse_native.h"

typedef struct { uint32_t mxcsr; uint32_t pad[3]; h2u128 xmm[8], out[4]; } Cpu;

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec * 1e9 + (double)t.tv_nsec;
}

#define ITERS 2000000
#define LOOP(name, body)                                                     \
    __attribute__((noinline)) static uint32_t loop_##name(Cpu *c) {        \
        uint32_t s = 0;                                                      \
        for (int i = 0; i < ITERS; ++i) {                                    \
            h2u128 a = c->xmm[i & 3], b = c->xmm[4 + (i & 3)], r = 0;        \
            s |= body;                                                       \
            c->out[i & 3] = r;                                               \
        }                                                                    \
        return s;                                                            \
    }
LOOP(addps, h2sn_addps(&c->mxcsr, a, b, &r))
LOOP(subps, h2sn_subps(&c->mxcsr, a, b, &r))
LOOP(mulps, h2sn_mulps(&c->mxcsr, a, b, &r))
LOOP(divps, h2sn_divps(&c->mxcsr, a, b, &r))
LOOP(sqrtps, h2sn_sqrtps(&c->mxcsr, b, &r))
LOOP(addss, h2sn_addss(&c->mxcsr, a, (uint32_t)b, &r))
LOOP(mulss, h2sn_mulss(&c->mxcsr, a, (uint32_t)b, &r))
LOOP(divss, h2sn_divss(&c->mxcsr, a, (uint32_t)b, &r))

static void reset(Cpu *c, int op) {
    static const float base[8] = {1.25f, -3.5f, 0.75f, 2.0f, 1.0001f, 0.9999f, -1.0f, 1.5f};
    c->mxcsr = 0x1f80u;
    for (int r = 0; r < 8; ++r) {
        h2u128 v = 0;
        for (int l = 0; l < 4; ++l) {
            float f = base[(r + l) & 7] * (op == 4 && r >= 4 ? (base[(r + l) & 7] < 0 ? -1.0f : 1.0f) : 1.0f);
            uint32_t u; memcpy(&u, &f, 4);
            v |= (h2u128)u << (32 * l);
        }
        c->xmm[r] = v;
    }
}

int main(void) {
    static Cpu c;
    struct { const char *name; uint32_t (*f)(Cpu *); } ops[] = {
        {"addps", loop_addps}, {"subps", loop_subps}, {"mulps", loop_mulps},
        {"divps", loop_divps}, {"sqrtps", loop_sqrtps}, {"addss", loop_addss},
        {"mulss", loop_mulss}, {"divss", loop_divss},
    };
    for (unsigned k = 0; k < sizeof ops / sizeof *ops; ++k) {
        double best = 1e30;
        uint32_t sink = 0;
        for (int run = 0; run < 15; ++run) {
            reset(&c, k == 4 ? 4 : 0);
            double t = now();
            sink |= ops[k].f(&c);
            t = now() - t;
            if (t < best) best = t;
        }
        printf("SSE_BENCH %s ns_per_op=%.2f status=%u\n", ops[k].name, best / ITERS, sink);
    }
    return 0;
}
