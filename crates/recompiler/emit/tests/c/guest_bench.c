/* Synthetic microbenchmarks. NATIVE selects old arithmetic/copy execution
 * versus guarded arithmetic and page-bounded REP MOVS. No game input. */
#define H2_FASTMEM 1
#define H2_ACCOUNTING 0
#include "h2fastmem.h"
#include "h2sse_native.h"
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static double cpu_time(void) {
    struct timespec t;
    clock_gettime(CLOCK_THREAD_CPUTIME_ID, &t);
    return (double)t.tv_sec + (double)t.tv_nsec * 1e-9;
}
#if NATIVE
#define SSE(name) h2sn_##name
#else
#define SSE(name) h2s_##name
#endif
__attribute__((noinline)) static h2u128 arithmetic(uint32_t *mx, h2u128 a, h2u128 b) {
    h2u128 r;
    SSE(addps)(mx, a, b, &r); SSE(mulps)(mx, r, b, &r);
    SSE(subps)(mx, r, b, &r); SSE(divps)(mx, r, b, &r);
    return r;
}
__attribute__((noinline)) static void copy(H2FastAccess *f, uint32_t count) {
#if NATIVE
    uint32_t n = h2_fast_movs(f, 0x1000, 0x2000, count, 4, 0, UINT64_MAX);
    if (n != count) abort();
#else
    for (uint32_t i = 0; i < count; i++) {
        uint8_t word[4];
        if (!h2_fast_copy(f, 0x1000 + i*4, 4, 1, word, NULL, 0)) abort();
        if (!h2_fast_copy(f, 0x2000 + i*4, 4, 1, NULL, word, 1)) abort();
    }
#endif
}
int main(void) {
    h2u128 a = 0, b = 0, signs_exp = 0, fractions = 0;
    uint32_t mx = 0x9f80;
    for (unsigned i = 0; i < 4; i++) {
        a |= (h2u128)(0x3fa00000u + i*713u) << (32*i);
        b |= (h2u128)(0x3f900000u + i*773u) << (32*i);
        signs_exp |= (h2u128)0x3f800000u << (32*i);
        fractions |= (h2u128)0x007fffffu << (32*i);
    }
    double start = cpu_time();
    for (unsigned i = 0; i < 2000000; i++) a = (arithmetic(&mx, a, b) & fractions) | signs_exp;
    printf("native=%d packed add/mul/sub/div cpu_ns/trip=%.3f checksum=%llx mx=%x\n",
           NATIVE, (cpu_time() - start)*500.0, (unsigned long long)a, mx);
    H2FastPage *pages = calloc(1u << 20, sizeof *pages);
    uint8_t *ram = calloc(1, 1u << 26);
    if (!pages || !ram) abort();
    pages[1] = (H2FastPage){0,3}; pages[2] = (H2FastPage){4096,3};
    for (unsigned i = 0; i < 4096; i++) ram[i] = (uint8_t)(i*71u+37u);
    H2MemoryShared m = {0}; m.data.pages = pages; m.data.ram = ram;
    uint32_t preempt = 0;
    H2FastAccess f = {&m,0,UINT64_MAX,&preempt};
    start = cpu_time();
    for (unsigned i = 0; i < 200000; i++) copy(&f,256);
    printf("native=%d 1KiB REP MOVSD cpu_ns/copy=%.3f accesses=%llu checksum=%u\n",
           NATIVE, (cpu_time()-start)*5000.0, (unsigned long long)f.accesses, ram[4096]);
    free(ram); free(pages);
    return 0;
}
