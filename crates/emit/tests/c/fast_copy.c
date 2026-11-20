#define H2_FASTMEM 1
#define H2_RAM_PAGE_CACHE 1
#define H2_FLAT_RAM 1
#include "h2fastmem.h"
#include <assert.h>
#include <stdlib.h>

/* Return the certified pointer without doing the actual copy. */
static uint8_t *site(H2FastAccess *f, H2Gate *g, uint32_t addr, uint32_t width, int write) {
    H2FastPage page;
    uint8_t *host;
    H2_SITE_TRY(f, (*g), addr, width, write, page, host, reject);
    return host;
reject:
    return NULL;
}

static void translated_spans(H2FastAccess *f, H2FastPage *pages, uint8_t *ram) {
    H2Gate g = {.dirtied = UINT32_MAX};
    H2MemoryData *d = &f->memory->data;
    /* Read slot at virtual page zero; write slot at the final page.
     * Both are aliases of physical page one. */
    pages[0] = (H2FastPage){4096, 1};
    pages[0xfffff] = (H2FastPage){4096, 2};
    h2_gate_adopt(&g, h2_gate_try(f));
    assert(site(f, &g, 0, 4, 0) == ram + 4096);
    assert(site(f, &g, 4080, 16, 0) == ram + 4096 + 4080);
    assert(site(f, &g, 4081, 16, 0) == NULL);
    assert(site(f, &g, 0, 4, 1) == NULL);
    assert(site(f, &g, 0xfffffff0u, 16, 1) == ram + 4096 + 4080);
    assert(site(f, &g, UINT32_MAX, 1, 1) == ram + 8191);
    assert(site(f, &g, UINT32_MAX, 2, 1) == NULL);
    assert(site(f, &g, 0xfffff000u, 1, 0) == NULL);
    assert(site(f, &g, 0, 4, 1) == NULL); /* no wrap into page zero */
    assert(d->dirty[0] == 2 && d->dirty_lo == 0 && d->dirty_hi == 1);
    assert(f->accesses == (H2_ACCOUNTING ? 0u : 4u));
    h2_gate_drop(f, &g);
    assert(site(f, &g, 0, 1, 0) == NULL);
    /* Drain, remap, and reverse permissions between two holds. No cached
     * permission, pointer or previous dirty mark can survive adoption. */
    d->dirty[0] = 0;
    d->dirty_lo = 256;
    d->dirty_hi = 0;
    pages[0] = (H2FastPage){0, 2};
    pages[0xfffff] = (H2FastPage){0, 1};
    h2_gate_adopt(&g, h2_gate_try(f));
    assert(site(f, &g, 0, 4, 0) == NULL);
    assert(site(f, &g, UINT32_MAX, 1, 1) == NULL);
    assert(site(f, &g, 4, 4, 1) == ram + 4);
    assert(site(f, &g, UINT32_MAX, 1, 0) == ram + 4095);
    assert(d->dirty[0] == 1);
    h2_gate_drop(f, &g);
    f->accesses = 0;
    d->dirty[0] = 0;
    d->dirty_lo = 256;
    d->dirty_hi = 0;
}

/* Old views may end immediately after the v1 prefix. A new handler must
 * negotiate the suffix before reading any of it. Span preflight neither
 * spends an access nor dirties a page, even with write rights. */
static void flat_spans(H2FastPage *pages, uint8_t *ram) {
    H2MemoryData old = {.pages = pages, .watching = 1};
    H2Gate g = {.held = 1, .data = &old};
    assert(!h2_flat_span(&g, 0, 4, 1));
    H2FlatMemoryData ext = {
        .prefix = {.pages = pages, .watching = UINT32_C(0x80000000)},
        .flat = {.base = ram}
    };
    g.data = &ext.prefix;
    pages[0] = (H2FastPage){4096, 8 | 3};
    assert(h2_flat_span(&g, 16, 32, 3) == ram + 16);
    assert(h2_flat_span(&g, 4095, 1, 3) == ram + 4095);
    assert(!h2_flat_span(&g, 4095, 2, 1));
    pages[0].permissions = 8 | 1;
    assert(!h2_flat_span(&g, 0, 4, 3));
    pages[0].permissions = 3;
    assert(!h2_flat_span(&g, 0, 4, 1));
    pages[0].permissions |= 8;
    ext.prefix.watching |= 1;
    assert(!h2_flat_span(&g, 0, 4, 2)); /* mirror setup failed */
    ext.flat.stores = ram;
    assert(h2_flat_span(&g, 16, 32, 3) == ram + 4096 + 16);
    assert(!ext.prefix.dirty[0]);
    g.held = 0;
    assert(!h2_flat_span(&g, 0, 4, 1));
}

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
    flat_spans(pages, ram);
    translated_spans(&f, pages, ram);
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
