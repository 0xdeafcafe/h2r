/* Optional host RAM view. No guest pointer is ever treated as a host pointer.
 * All RAM/page-table/dirty-bit accesses hold the gate shared with Rust.
 * Failed fast checks release it before calling the ordinary ABI helper. */
#ifndef H2FASTMEM_H
#define H2FASTMEM_H
#include "h2abi.h"
#include <string.h>
typedef struct H2FastPage { uint32_t physical, permissions; } H2FastPage;
typedef struct H2MemoryData {
    const H2FastPage *pages;
    uint8_t *ram;
    uint64_t dirty[256];
    uint32_t dirty_lo, dirty_hi, watching;
} H2MemoryData;
typedef struct H2MemoryShared { uint32_t gate; H2MemoryData data; } H2MemoryShared;
struct H2FastAccess {
    H2MemoryShared *memory;
    uint64_t accesses, budget;
    uint32_t *preempt;
};
#if UINTPTR_MAX == UINT64_MAX
_Static_assert(sizeof(H2MemoryData) == 2080 && sizeof(H2MemoryShared) == 2088, "fast memory layout");
_Static_assert(offsetof(H2MemoryShared, data) == 8 && sizeof(H2FastAccess) == 32, "fast context layout");
#endif
#ifndef H2_ACCOUNTING
#define H2_ACCOUNTING 0
#endif
/* With access accounting (h2gen.h) the handler counts every attempted access
 * itself, fast or slow, so the fast path charges nothing and the slow path
 * takes the uncharged accessors: nothing is charged twice. */
#if H2_ACCOUNTING
#define H2_FAST_CHARGE(f) ((void)(f))
#define H2_MEM_READ_SLOW h2abi_v1_read_counted
#define H2_MEM_WRITE_SLOW h2abi_v1_write_counted
#else
#define H2_FAST_CHARGE(f) h2_fast_charge(f)
#define H2_MEM_READ_SLOW h2abi_v1_read
#define H2_MEM_WRITE_SLOW h2abi_v1_write
#endif
#if H2_FASTMEM
#define H2_FAST_LOCALS() H2FastAccess *h2_mem_ = h2abi_v1_fast_acquire(ctx); (void)h2_mem_
#define H2_FAST_RELEASE() h2abi_v1_fast_release(ctx)
#else
#define H2_FAST_LOCALS() H2FastAccess *h2_mem_ = NULL; (void)h2_mem_
#define H2_FAST_RELEASE() ((void)0)
#endif

static inline void h2_fast_charge(H2FastAccess *f) {
    if (++f->accesses == f->budget)
        __atomic_fetch_or(f->preempt, 1u, __ATOMIC_RELEASE); /* SLICE */
}
/* read/write are constants at emitted call sites. Width/alignment remain
 * checked here, so even a nonstandard caller cannot bypass the slow rules. */
static inline int h2_fast_copy(H2FastAccess *f, uint32_t addr, uint32_t width,
                              uint32_t alignment, uint8_t *out,
                              const uint8_t *value, int write) {
    if (!f || (write ? !value : !out) || !(width == 1u || width == 2u || width == 4u || width == 8u || width == 10u || width == 16u)
        || !alignment || (alignment & (alignment - 1u)) || (addr & (alignment - 1u))
        || (addr & 4095u) > 4096u - width) return 0;
    H2MemoryShared *m = f->memory;
    uint32_t unlocked = 0;
    if (!__atomic_compare_exchange_n(&m->gate, &unlocked, 1u, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return 0;
    H2MemoryData *d = &m->data;
    if (!d->pages) {
        __atomic_store_n(&m->gate, 0u, __ATOMIC_RELEASE);
        return 0;
    }
    H2FastPage page = d->pages[addr >> 12];
    if (!(page.permissions & (write ? 2u : 1u)) || page.physical > 0x04000000u - 4096u) {
        __atomic_store_n(&m->gate, 0u, __ATOMIC_RELEASE);
        return 0;
    }
    uint8_t *ram = d->ram + page.physical + (addr & 4095u);
    if (write) {
        memcpy(ram, value, width);
        if (d->watching) {
            uint32_t p = page.physical >> 12, word = p >> 6;
            d->dirty[word] |= UINT64_C(1) << (p & 63u);
            if (word < d->dirty_lo) d->dirty_lo = word;
            if (word + 1u > d->dirty_hi) d->dirty_hi = word + 1u;
        }
    } else {
        memcpy(out, ram, width);
    }
    __atomic_store_n(&m->gate, 0u, __ATOMIC_RELEASE);
    H2_FAST_CHARGE(f);
    return 1;
}
/* A bounded REP MOVS chunk. Both spans stay within their current RAM page;
 * rejection performs no accesses, so the ordinary iteration retains faults,
 * MMIO and partial progress. `limit` is the next accounting safe point. */
static inline uint32_t h2_fast_movs(H2FastAccess *f, uint32_t src, uint32_t dst,
                                  uint32_t count, uint32_t width, int reverse,
                                  uint64_t limit) {
#if H2_FASTMEM
    if (!f || !count || !(width == 1u || width == 2u || width == 4u)) return 0;
    uint32_t so = src & 4095u, doff = dst & 4095u;
    if (so > 4096u - width || doff > 4096u - width) return 0;
    uint32_t sn = reverse ? so / width + 1u : (4096u - so) / width;
    uint32_t dn = reverse ? doff / width + 1u : (4096u - doff) / width;
    uint32_t n = count < sn ? count : sn;
    if (n > dn) n = dn;
    uint64_t iterations = limit / 2u + (limit & 1u);
    if (!iterations) iterations = 1u; /* the original loop polls after a trip */
    if (n > iterations) n = (uint32_t)iterations;
#if H2_ACCOUNTING
    if (f->preempt && __atomic_load_n(f->preempt, __ATOMIC_RELAXED)) n = 1u;
#endif
    H2MemoryShared *m = f->memory;
    uint32_t unlocked = 0;
    if (!__atomic_compare_exchange_n(&m->gate, &unlocked, 1u, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return 0;
    H2MemoryData *d = &m->data;
    if (!d->pages) { __atomic_store_n(&m->gate, 0u, __ATOMIC_RELEASE); return 0; }
    H2FastPage sp = d->pages[src >> 12], dp = d->pages[dst >> 12];
    if (!(sp.permissions & 1u) || !(dp.permissions & 2u) ||
        sp.physical > 0x04000000u - 4096u || dp.physical > 0x04000000u - 4096u) {
        __atomic_store_n(&m->gate, 0u, __ATOMIC_RELEASE); return 0;
    }
    uint32_t from = sp.physical + so, to = dp.physical + doff, bytes = n * width;
    if (reverse) { from -= bytes - width; to -= bytes - width; }
    /* Propagating overlap is element-ordered x86 copying, not memmove.
     * Compare physical spans: virtual aliases must not hide overlap. */
    if (from < to + bytes && to < from + bytes && from != to &&
        (reverse ? to < from : to > from)) {
        __atomic_store_n(&m->gate, 0u, __ATOMIC_RELEASE); return 0;
    }
    memmove(d->ram + to, d->ram + from, bytes);
    if (d->watching) {
        uint32_t p = dp.physical >> 12, word = p >> 6;
        d->dirty[word] |= UINT64_C(1) << (p & 63u);
        if (word < d->dirty_lo) d->dirty_lo = word;
        if (word + 1u > d->dirty_hi) d->dirty_hi = word + 1u;
    }
    __atomic_store_n(&m->gate, 0u, __ATOMIC_RELEASE);
#if !H2_ACCOUNTING
    uint64_t previous = f->accesses, charged = (uint64_t)n * 2u;
    f->accesses += charged;
    /* Distance to the first matching increment, including unsigned wrap. */
    if (f->budget - previous - 1u < charged)
        __atomic_fetch_or(f->preempt, 1u, __ATOMIC_RELEASE);
#endif
    return n;
#else
    (void)f; (void)src; (void)dst; (void)count; (void)width; (void)reverse; (void)limit;
    return 0;
#endif
}
static inline uint32_t h2_mem_read(H2FastAccess *f, ExecCtxOpaque *ctx, uint32_t pc,
                                 uint32_t addr, uint32_t width, uint32_t alignment, uint8_t *out) {
#if H2_FASTMEM
    if (h2_fast_copy(f, addr, width, alignment, out, NULL, 0)) return 0u;
#else
    (void)f;
#endif
    return H2_MEM_READ_SLOW(ctx, pc, addr, width, alignment, out);
}
static inline uint32_t h2_mem_write(H2FastAccess *f, ExecCtxOpaque *ctx, uint32_t pc,
                                  uint32_t addr, uint32_t width, uint32_t alignment, const uint8_t *value) {
#if H2_FASTMEM
    if (h2_fast_copy(f, addr, width, alignment, NULL, value, 1)) return 0u;
#else
    (void)f;
#endif
    return H2_MEM_WRITE_SLOW(ctx, pc, addr, width, alignment, value);
}
#endif
