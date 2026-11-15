#define H2_FASTMEM 1
#include "h2fastmem.h"
#include <assert.h>
#include <stdlib.h>

int main(void) {
    H2FastPage *pages = calloc(1u << 20, sizeof *pages);
    uint8_t *ram = malloc(1u << 26);
    assert(pages && ram);
    pages[1] = (H2FastPage){0,3}; pages[2] = (H2FastPage){4096,3};
    pages[3] = pages[1]; /* a virtual alias */
    H2MemoryShared m = {0};
    m.data.pages = pages; m.data.ram = ram; m.data.watching = 1;
    m.data.dirty_lo = 256;
    uint32_t preempt = 0;
    H2FastAccess f = {&m, 0, 100, &preempt};
    for (unsigned width = 1; width <= 4; width *= 2) {
        for (int reverse = 0; reverse <= 1; reverse++) {
            for (unsigned delta = 0; delta <= 9; delta++) {
                for (int dir = -1; dir <= 1; dir += 2) {
                    uint8_t expected[4096];
                    for (unsigned i = 0; i < 4096; i++) expected[i] = ram[i] = (uint8_t)(i*71u+37u);
                    uint32_t src = 512u, dst = (uint32_t)(512 + dir * (int)delta), count = 32;
                    for (unsigned i = 0; i < count; i++) {
                        uint8_t word[4];
                        unsigned off = i * width;
                        unsigned a = reverse ? src-off : src+off, b = reverse ? dst-off : dst+off;
                        memcpy(word, expected+a, width); memcpy(expected+b, word, width);
                    }
                    f.accesses = 0; preempt = 0;
                    uint32_t n = h2_fast_movs(&f, 0x1000+src, 0x3000+dst, count, width, reverse, UINT64_MAX);
                    if (n) {
                        assert(n == count);
                        assert(!memcmp(expected, ram, sizeof expected));
#if !H2_ACCOUNTING
                        assert(f.accesses == 2u*count);
#else
                        assert(f.accesses == 0);
#endif
                        assert(m.data.dirty[0] & 1u);
                    } else {
                        /* Reject propagating overlap without any side effects. */
                        assert(delta && delta < count*width);
                        assert(reverse ? dst < src : dst > src);
                        assert(f.accesses == 0);
                        for (unsigned i = 0; i < 4096; i++) assert(ram[i] == (uint8_t)(i*71u+37u));
                    }
                }
            }
        }
    }
    f.accesses = 0; preempt = 0;
    assert(h2_fast_movs(&f,0x1ff8,0x2000,100,4,0,UINT64_MAX) == 2);
    assert(h2_fast_movs(&f,0x1000,0x2000,100,4,0,3) == 2);
    assert(h2_fast_movs(&f,0x1000,0x2000,0,4,0,100) == 0);
    assert(h2_fast_movs(&f,0x1fff,0x2000,3,4,0,100) == 0);
    assert(h2_fast_movs(&f,0xfd000000,0x2000,3,4,0,100) == 0);
    assert(h2_fast_movs(&f,0x1000,0x4000,3,4,0,100) == 0);
    pages[2].permissions = 1;
    assert(h2_fast_movs(&f,0x1000,0x2000,3,4,0,100) == 0);
    pages[2].permissions = 3;
    m.gate = 1;
    assert(h2_fast_movs(&f,0x1000,0x2000,3,4,0,100) == 0);
    assert(m.gate == 1);
    m.gate = 0;
    f.accesses = 0; f.budget = 3; preempt = 0;
    assert(h2_fast_movs(&f,0x1000,0x2000,3,4,0,3) == 2);
#if !H2_ACCOUNTING
    assert(f.accesses == 4 && preempt == 1);
    f.accesses = UINT64_MAX - 1u; f.budget = 0; preempt = 0;
    assert(h2_fast_movs(&f,0x1000,0x2000,2,4,0,4) == 2);
    assert(f.accesses == 2 && preempt == 1);
#else
    assert(f.accesses == 0 && preempt == 0);
    preempt = 4;
    assert(h2_fast_movs(&f,0x1000,0x2000,100,4,0,UINT64_MAX) == 1);
#endif
    free(ram); free(pages);
    return 0;
}
