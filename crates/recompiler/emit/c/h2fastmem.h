/* Inline guest memory, in one of two models.
 *
 * Paged (H2_FASTMEM without H2_FLAT_RAM): an optional host RAM view. No
 * guest pointer is ever treated as a host pointer. All RAM/page-table/
 * dirty-bit accesses hold the gate shared with Rust. Failed fast checks
 * release it before calling the ordinary ABI helper.
 *
 * Flat (H2_FLAT_RAM with H2_FASTMEM, H2_FLAT_ below; module feature 16):
 * every access is one host instruction at the runtime's window base plus
 * the guest address, with a fixup row that sends a faulting one to the
 * same helper (docs/runtime/contract-thin-transfers-flat-ram.md, section
 * 4.1, slice F3). No gate, no page table. */
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
/* Which model. H2_FLAT_SPAN is the opt-in span image (H2_EMIT_FLAT_RAM=1,
 * the M0 measurement's), on the paged model; it goes when M0 is taken. */
#ifndef H2_FLAT_RAM
#define H2_FLAT_RAM 0
#endif
#ifndef H2_FLAT_SPAN
#define H2_FLAT_SPAN 0
#endif
#if H2_FLAT_RAM && H2_FASTMEM
#define H2_FLAT_ 1
#else
#define H2_FLAT_ 0
#endif
#if H2_FLAT_ && H2_FLAT_SPAN
#error "H2_FLAT_RAM and H2_FLAT_SPAN are two memory models: build one"
#endif
#if H2_FLAT_ && (defined(_WIN32) || !(defined(__aarch64__) || defined(__x86_64__)))
#error "H2_FLAT_RAM: this target has no flat RAM window yet (contract slice F7)"
#endif
/* The flat model's per-context block: the context's third word points to
 * it (runtime ffi/mem_block.rs). `base` is the host address of guest
 * address 0, fixed for the runtime, or a window with nothing mapped for a
 * context whose fast memory is off; `cards` has one byte per 4 KiB guest
 * page (0: written), and one per 4 MiB region at cards - 1024. The count
 * and budget work as H2FastAccess's: the runtime sets the budget before it
 * invokes a handler and settles the count after. A handler loads base and
 * cards once, at entry. */
typedef struct H2Mem {
    uint8_t *base;
    uint8_t *cards;
    uint64_t accesses, budget;
    uint32_t *preempt;
} H2Mem;
#if UINTPTR_MAX == UINT64_MAX
_Static_assert(sizeof(H2Mem) == 40, "flat context layout");
#endif
/* What a handler counts and charges in: its view, or its block. */
#if H2_FLAT_
typedef H2Mem H2View;
#else
typedef H2FastAccess H2View;
#endif
/* Preempt word bit 16, IRQ_QUEUED (runtime preempt.rs): set while the
 * interrupt queue is non-empty, not a wake reason. Polls mask it out; the
 * direct-call quiet test reads the whole word (h2gen.h, module feature
 * H2GEN_FEATURE_ENTRY_TABLE). */
#define H2_IRQ_QUEUED 16u
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
/* Optional coverage for one generated owner, including cold gate retries.
 * Count a rejected preflight only when its individual accesses execute. */
#ifdef H2_FLAT_STATS_OWNER
#include <stdio.h>
enum { H2FC_TOTAL, H2FC_FLAT, H2FC_BULK, H2FC_COUNT };
extern H2_EXPORT uint64_t h2gen_flat_counts[H2FC_COUNT];
#define H2_FLAT_OWNER(pc) const int h2_count_flat_ __attribute__((unused)) = (pc) == H2_FLAT_STATS_OWNER
#define H2_FLAT_NOTE(kind, n)                                                   \
    do {                                                                        \
        if (h2_count_flat_)                                                     \
            __atomic_fetch_add(&h2gen_flat_counts[kind], (uint64_t)(n),         \
                               __ATOMIC_RELAXED);                              \
    } while (0)

#ifdef H2_FLAT_STATS_REGISTRY
H2_EXPORT uint64_t h2gen_flat_counts[H2FC_COUNT];
static __attribute__((destructor)) void h2_flat_report(void) {
    uint64_t n = __atomic_load_n(&h2gen_flat_counts[H2FC_TOTAL], __ATOMIC_RELAXED);
    uint64_t flat = __atomic_load_n(&h2gen_flat_counts[H2FC_FLAT], __ATOMIC_RELAXED);
    uint64_t bulk = __atomic_load_n(&h2gen_flat_counts[H2FC_BULK], __ATOMIC_RELAXED);
    fprintf(stderr, "flat owner %08x: %llu accesses, %llu flat, %llu bulk, %llu checked\n",
            (unsigned)H2_FLAT_STATS_OWNER, (unsigned long long)n, (unsigned long long)flat,
            (unsigned long long)bulk, (unsigned long long)(n - flat - bulk));
}
#endif
#define H2_COLD_FLAT(r) ((r).flat = 1)
#define H2_NOTE_COLD_FLAT(r) H2_FLAT_NOTE(H2FC_FLAT, (r).flat)
#else
#define H2_FLAT_OWNER(pc) ((void)0)
#define H2_FLAT_NOTE(kind, n) ((void)0)
#define H2_COLD_FLAT(r) ((void)0)
#define H2_NOTE_COLD_FLAT(r) ((void)0)
#endif

/* The gate word: bit 0 is the holder (C or a Rust memory guard), bit 1 a
 * Rust guard waiting for it. C only ever takes a gate that is entirely
 * clear and releases bit 0 alone, so a waiting guard is next: C's following
 * access fails its try and blocks in the slow helper behind that guard. */
#define H2_GATE_HELD 1u
#define H2_GATE_WANTED 2u
#define H2_GATE_RELEASE(m) ((void)__atomic_fetch_and(&(m)->gate, ~H2_GATE_HELD, __ATOMIC_RELEASE))
/* A handler's hold on the gate, kept across consecutive inline accesses
 * (H2_HELD_TRY) instead of taking and releasing it for each. `held` is 0
 * when the gate is not held, else 1 plus the loop headers passed since it
 * was taken. `data` is the view it was taken with: its page table, RAM
 * pointer and watch flag cannot change while the gate is held, and none of
 * it is used after the gate is dropped. The handler drops it (H2_GATE_DROP
 * in h2gen.h) before every call that takes the context, on every way out,
 * and after H2_GATE_TRIPS loop headers or as soon as a Rust guard wants it.
 * `dirtied` is the physical page this hold last marked dirty (all ones:
 * none): Rust drains dirty bits only under the gate, so while it is held a
 * mark stays set and another store to that page need not repeat it. */
/* H2_COLD_STATS=1 (emit-time, default off): count every call of
 * h2_access_cold by why the access was not inline, in the module-wide
 * h2gen_cold_counts, and print them to stderr every 2^22 calls. With the
 * gate held the inline check failed (H2C_MISS: a page crossing, no
 * permission, MMIO, an odd width). Without it, the reason the hold ended:
 * the handler's start, called by the runner (H2C_ENTRY) or directly with a
 * lent view (H2C_LENT_ENTRY); back from a direct call (H2C_AFTER_DCALL);
 * the access count settled (H2C_FLUSH); a runtime call, atomic or event
 * (H2C_DROP); a loop header ending the hold (H2C_TICK). H2C_NO_GATE counts,
 * on top, the calls that then could not take the gate (busy, no RAM view)
 * or failed the inline check under it, and went to the helper. */
#ifndef H2_COLD_STATS
#define H2_COLD_STATS 0
#endif
enum {
    H2C_ENTRY, H2C_LENT_ENTRY, H2C_AFTER_DCALL, H2C_FLUSH, H2C_DROP, H2C_TICK, H2C_MISS, H2C_NO_GATE,
    H2C_COUNT
};
#if H2_RAM_PAGE_CACHE && H2_FASTMEM
/* Private to a generated handler. A write slot also certifies that the
 * physical page was marked dirty if a watch was active in this hold. */
typedef struct H2RamPage {
    uint64_t end; /* exclusive guest end, 0 for an empty slot */
    uint8_t *base;
} H2RamPage;
#endif
typedef struct H2Gate {
    uint32_t held, dirtied;
    H2MemoryData *data;
#if H2_COLD_STATS
    uint32_t why; /* H2C_*: why the gate is not held */
#endif
#if H2_RAM_PAGE_CACHE && H2_FASTMEM
    H2RamPage ram_page[2]; /* last read and last write are independent */
#endif
} H2Gate;
#define H2_GATE_TRIPS 32u
#define H2_ALWAYS_INLINE inline __attribute__((always_inline))
#define H2_COLD __attribute__((noinline, cold, unused))
#define H2_LIKELY(x) __builtin_expect(!!(x), 1)
/* Out-of-line helpers called from every access site keep the caller's
 * registers, so the site spills nothing around the call. */
#if defined(__aarch64__) || defined(__x86_64__)
#define H2_PRESERVE __attribute__((preserve_most))
#else
#define H2_PRESERVE
#endif
#if H2_COLD_STATS
#include <stdio.h>
/* One array for the whole module: every object's weak definition merges. */
__attribute__((weak, visibility("default"))) uint64_t h2gen_cold_counts[H2C_COUNT];
static H2_COLD H2_PRESERVE void h2_cold_note(uint32_t why) {
    static const char *const names[H2C_COUNT] = {
        "entry", "lent-entry", "after-dcall", "flush", "drop", "tick", "miss", "no-gate",
    };
    __atomic_fetch_add(&h2gen_cold_counts[why], 1u, __ATOMIC_RELAXED);
    if (why == H2C_NO_GATE) return;
    uint64_t total = 0;
    for (uint32_t i = 0; i < H2C_NO_GATE; i++) total += __atomic_load_n(&h2gen_cold_counts[i], __ATOMIC_RELAXED);
    if (total & ((UINT64_C(1) << 22) - 1u)) return;
    fprintf(stderr, "h2gen cold accesses: %llu", (unsigned long long)total);
    for (uint32_t i = 0; i < H2C_COUNT; i++)
        fprintf(stderr, " %s %llu", names[i], (unsigned long long)__atomic_load_n(&h2gen_cold_counts[i], __ATOMIC_RELAXED));
    fprintf(stderr, "\n");
}
#define H2_COLD_WHY(g, w) ((g).why = (w))
/* Before h2_access_cold, with the gate state the site saw. */
#define H2_COLD_NOTE(g) h2_cold_note((g).held ? (uint32_t)H2C_MISS : (g).why)
/* After it: no gate now means the helper ran, and the next cold access is
 * the miss's doing. */
#define H2_COLD_NOTE_AFTER(g)                                                   \
    do {                                                                        \
        if (!(g).held) {                                                        \
            h2_cold_note(H2C_NO_GATE);                                          \
            (g).why = H2C_MISS;                                                 \
        }                                                                       \
    } while (0)
#else
#define H2_COLD_WHY(g, w) ((void)0)
#define H2_COLD_NOTE(g) ((void)0)
#define H2_COLD_NOTE_AFTER(g) ((void)0)
#endif
#if H2_FLAT_
/* The context's third word: the flat block. Never null for a flat module
 * (the runtime and the emitter's test host both set it). */
static inline H2Mem *h2_mem_block(ExecCtxOpaque *ctx) {
    H2Mem *m;
    memcpy(&m, (const char *)ctx + 2 * sizeof(void *), sizeof m);
    return m;
}
#define H2_FAST_LOCALS()                                                        \
    H2Mem *const h2_mem_ = h2_mem_block(ctx);                                   \
    uint8_t *const h2_base_ = h2_mem_->base;                                    \
    uint8_t *const h2_cards_ = h2_mem_->cards;                                  \
    H2Gate h2g_ = {.dirtied = UINT32_MAX};                                      \
    (void)h2_mem_; (void)h2_base_; (void)h2_cards_; (void)h2g_
#define H2_FAST_RELEASE() ((void)0)
#elif H2_FASTMEM
#define H2_FAST_LOCALS() H2FastAccess *h2_mem_ = h2abi_v1_fast_acquire(ctx); H2Gate h2g_ = {.dirtied = UINT32_MAX}; (void)h2_mem_
#define H2_FAST_RELEASE() h2abi_v1_fast_release(ctx)
#else
#define H2_FAST_LOCALS() H2FastAccess *h2_mem_ = NULL; H2Gate h2g_ = {.dirtied = UINT32_MAX}; (void)h2_mem_
#define H2_FAST_RELEASE() ((void)0)
#endif

static inline void h2_fast_charge(H2View *f) {
    if (++f->accesses == f->budget)
        __atomic_fetch_or(f->preempt, 1u, __ATOMIC_RELEASE); /* SLICE */
}
#if !H2_FLAT_
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
        H2_GATE_RELEASE(m);
        return 0;
    }
    H2FastPage page = d->pages[addr >> 12];
    if (!(page.permissions & (write ? 2u : 1u)) || page.physical > 0x04000000u - 4096u) {
        H2_GATE_RELEASE(m);
        return 0;
    }
    uint8_t *ram = d->ram + page.physical + (addr & 4095u);
    if (write) {
        memcpy(ram, value, width);
        if (d->watching & 1u) {
            uint32_t p = page.physical >> 12, word = p >> 6;
            d->dirty[word] |= UINT64_C(1) << (p & 63u);
            if (word < d->dirty_lo) d->dirty_lo = word;
            if (word + 1u > d->dirty_hi) d->dirty_hi = word + 1u;
        }
    } else {
        memcpy(out, ram, width);
    }
    H2_GATE_RELEASE(m);
    H2_FAST_CHARGE(f);
    return 1;
}
#endif
#if H2_FLAT_
/* ---- The flat model's accesses (section 4.1, "An access is one host
 * instruction"). Each is an `asm goto` statement: one load or store at
 * base + (uint64_t)addr, and a row in the module's fixup table (section
 * h2_fix) giving the instruction's address and the site's `slow_` label,
 * as two 32-bit self-relative words (hostfault::Format::Relative32). A
 * fault on the instruction resumes at `slow_` (the runtime's window
 * region, hostfault::fixup), where the site calls the checked helper:
 * fault order, whole-span validation, MMIO and the access trap stay the
 * helper's. Every site using these has a `__label__ slow_` in scope.
 *
 * The address wraps at 32 bits by construction, and a span that runs off
 * the end of the window lands in its guard. A span crossing a 4 KiB page
 * is native when both pages are in the window. The statement reads or
 * writes memory ("memory" clobber), so the compiler keeps guest order
 * between accesses through different addresses: no fence is needed.
 *
 * Ten bytes are two instructions, two rows and one label. A 10-byte store
 * whose second half faults has written its first eight bytes when the
 * helper runs: the helper then writes all ten or faults, and a fault there
 * is a guest crash, which the paged model would have left unwritten. */
#if defined(__APPLE__)
#define H2_FIX_PUSH ".pushsection __DATA_CONST,__h2_fix,regular\n"
#else
/* A C identifier, so the linker defines __start_h2_fix and __stop_h2_fix. */
#define H2_FIX_PUSH ".pushsection h2_fix,\"a\"\n"
#endif
#define H2_FIX_ROW(insn) H2_FIX_PUSH ".p2align 2\n.long " insn " - .\n.long %l[slow_] - .\n.popsection\n"
/* The table's bounds, for the module table (h2gen.h H2GenModule). The
 * rows are in address order as long as the module's text is: every row is
 * emitted right after its instruction, and the runtime refuses an unsorted
 * table by name (RegistrationError::FixupTable) rather than sort it. */
#if defined(__APPLE__)
extern const char h2_fix_start_[] __asm("section$start$__DATA_CONST$__h2_fix");
extern const char h2_fix_end_[] __asm("section$end$__DATA_CONST$__h2_fix");
#else
extern const char __start_h2_fix[] __attribute__((weak, visibility("hidden")));
extern const char __stop_h2_fix[] __attribute__((weak, visibility("hidden")));
#define h2_fix_start_ __start_h2_fix
#define h2_fix_end_ __stop_h2_fix
#endif
#define H2_FIX_BOUNDS (const void *)h2_fix_start_, (const void *)h2_fix_end_, 0u
typedef uint8_t h2_flat_v16 __attribute__((vector_size(16)));

#if defined(__aarch64__)
#define H2_FLAT_LDW_(insn, dst, base, a)                                        \
    do {                                                                        \
        uint32_t v_;                                                            \
        __asm__ goto("1: " insn " %w[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("1b") \
                     : [v] "=r"(v_) : [b] "r"(base), [a] "r"(a) : "memory" : slow_); \
        (dst) = v_;                                                             \
    } while (0)
#define H2_FLAT_STW_(insn, v, base, a)                                          \
    __asm__ goto("1: " insn " %w[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("1b")    \
                 : : [v] "r"((uint32_t)(v)), [b] "r"(base), [a] "r"(a) : "memory" : slow_)
#define H2_FLAT_LD_1(dst, base, a) H2_FLAT_LDW_("ldrb", dst, base, a)
#define H2_FLAT_LD_2(dst, base, a) H2_FLAT_LDW_("ldrh", dst, base, a)
#define H2_FLAT_LD_4(dst, base, a) H2_FLAT_LDW_("ldr", dst, base, a)
#define H2_FLAT_ST_1(v, base, a) H2_FLAT_STW_("strb", v, base, a)
#define H2_FLAT_ST_2(v, base, a) H2_FLAT_STW_("strh", v, base, a)
#define H2_FLAT_ST_4(v, base, a) H2_FLAT_STW_("str", v, base, a)
#define H2_FLAT_LD_8(dst, base, a)                                              \
    do {                                                                        \
        uint64_t v_;                                                            \
        __asm__ goto("1: ldr %x[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("1b")     \
                     : [v] "=r"(v_) : [b] "r"(base), [a] "r"(a) : "memory" : slow_); \
        (dst) = v_;                                                             \
    } while (0)
#define H2_FLAT_ST_8(v, base, a)                                                \
    __asm__ goto("1: str %x[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("1b")         \
                 : : [v] "r"((uint64_t)(v)), [b] "r"(base), [a] "r"(a) : "memory" : slow_)
#define H2_FLAT_LD_16(dst, base, a)                                             \
    do {                                                                        \
        h2_flat_v16 v_;                                                         \
        __asm__ goto("1: ldr %q[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("1b")     \
                     : [v] "=w"(v_) : [b] "r"(base), [a] "r"(a) : "memory" : slow_); \
        memcpy(&(dst), &v_, 16);                                                \
    } while (0)
#define H2_FLAT_ST_16(v, base, a)                                               \
    do {                                                                        \
        h2_flat_v16 x_;                                                         \
        memcpy(&x_, &(v), 16);                                                  \
        __asm__ goto("1: str %q[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("1b")     \
                     : : [v] "w"(x_), [b] "r"(base), [a] "r"(a) : "memory" : slow_); \
    } while (0)
#define H2_FLAT_LD_10(dst, base, a)                                             \
    do {                                                                        \
        uint64_t lo_;                                                           \
        uint32_t hi_;                                                           \
        const uint8_t *t_;                                                      \
        __asm__ goto("add %[t], %[b], %w[a], uxtw\n"                            \
                     "1: ldr %x[lo], [%[t]]\n" H2_FIX_ROW("1b")                 \
                     "2: ldrh %w[hi], [%[t], #8]\n" H2_FIX_ROW("2b")            \
                     : [lo] "=&r"(lo_), [hi] "=&r"(hi_), [t] "=&r"(t_)          \
                     : [b] "r"(base), [a] "r"(a) : "memory" : slow_);           \
        (dst) = (unsigned __int128)lo_ | (unsigned __int128)hi_ << 64;                              \
    } while (0)
#define H2_FLAT_ST_10(v, base, a)                                               \
    do {                                                                        \
        uint64_t lo_ = (uint64_t)(v);                                           \
        uint32_t hi_ = (uint32_t)((v) >> 64);                                   \
        uint8_t *t_;                                                            \
        __asm__ goto("add %[t], %[b], %w[a], uxtw\n"                            \
                     "1: str %x[lo], [%[t]]\n" H2_FIX_ROW("1b")                 \
                     "2: strh %w[hi], [%[t], #8]\n" H2_FIX_ROW("2b")            \
                     : [t] "=&r"(t_)                                            \
                     : [lo] "r"(lo_), [hi] "r"(hi_), [b] "r"(base), [a] "r"(a)  \
                     : "memory" : slow_);                                       \
    } while (0)
/* A bulk copy's probe of one byte: a load, or a load and a store of the
 * same byte. */
#define H2_FLAT_PROBE_R_(base, a)                                               \
    do {                                                                        \
        uint32_t v_;                                                            \
        __asm__ goto("1: ldrb %w[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("1b")    \
                     : [v] "=r"(v_) : [b] "r"(base), [a] "r"(a) : "memory" : slow_); \
        (void)v_;                                                               \
    } while (0)
#define H2_FLAT_PROBE_RW_(base, a)                                              \
    do {                                                                        \
        uint32_t v_;                                                            \
        __asm__ goto("1: ldrb %w[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("1b")    \
                     "2: strb %w[v], [%[b], %w[a], uxtw]\n" H2_FIX_ROW("2b")    \
                     : [v] "=&r"(v_) : [b] "r"(base), [a] "r"(a) : "memory" : slow_); \
        (void)v_;                                                               \
    } while (0)
#elif defined(__x86_64__)
typedef long long h2_flat_x16 __attribute__((vector_size(16)));
/* The address operand is the 32-bit guest address zero-extended. */
#define H2_FLAT_LDX_(insn, reg, dst, base, a)                                   \
    do {                                                                        \
        uint64_t v_;                                                            \
        __asm__ goto("1: " insn " (%[b],%[a]), %" reg "[v]\n" H2_FIX_ROW("1b")  \
                     : [v] "=r"(v_) : [b] "r"(base), [a] "r"((uint64_t)(a)) : "memory" : slow_); \
        (dst) = v_;                                                             \
    } while (0)
#define H2_FLAT_STX_(insn, reg, v, base, a)                                     \
    __asm__ goto("1: " insn " %" reg "[v], (%[b],%[a])\n" H2_FIX_ROW("1b")      \
                 : : [v] "r"((uint64_t)(v)), [b] "r"(base), [a] "r"((uint64_t)(a)) : "memory" : slow_)
/* A 32-bit destination zero-extends into the whole register. */
#define H2_FLAT_LD_1(dst, base, a) H2_FLAT_LDX_("movzbl", "k", dst, base, a)
#define H2_FLAT_LD_2(dst, base, a) H2_FLAT_LDX_("movzwl", "k", dst, base, a)
#define H2_FLAT_LD_4(dst, base, a) H2_FLAT_LDX_("movl", "k", dst, base, a)
#define H2_FLAT_LD_8(dst, base, a) H2_FLAT_LDX_("movq", "q", dst, base, a)
#define H2_FLAT_ST_1(v, base, a) H2_FLAT_STX_("movb", "b", v, base, a)
#define H2_FLAT_ST_2(v, base, a) H2_FLAT_STX_("movw", "w", v, base, a)
#define H2_FLAT_ST_4(v, base, a) H2_FLAT_STX_("movl", "k", v, base, a)
#define H2_FLAT_ST_8(v, base, a) H2_FLAT_STX_("movq", "q", v, base, a)
#define H2_FLAT_LD_16(dst, base, a)                                             \
    do {                                                                        \
        h2_flat_x16 v_;                                                         \
        __asm__ goto("1: movdqu (%[b],%[a]), %[v]\n" H2_FIX_ROW("1b")           \
                     : [v] "=x"(v_) : [b] "r"(base), [a] "r"((uint64_t)(a)) : "memory" : slow_); \
        memcpy(&(dst), &v_, 16);                                                \
    } while (0)
#define H2_FLAT_ST_16(v, base, a)                                               \
    do {                                                                        \
        h2_flat_x16 x_;                                                         \
        memcpy(&x_, &(v), 16);                                                  \
        __asm__ goto("1: movdqu %[v], (%[b],%[a])\n" H2_FIX_ROW("1b")           \
                     : : [v] "x"(x_), [b] "r"(base), [a] "r"((uint64_t)(a)) : "memory" : slow_); \
    } while (0)
#define H2_FLAT_LD_10(dst, base, a)                                             \
    do {                                                                        \
        uint64_t lo_, hi_;                                                      \
        __asm__ goto("1: movq (%[b],%[a]), %q[lo]\n" H2_FIX_ROW("1b")           \
                     "2: movzwl 8(%[b],%[a]), %k[hi]\n" H2_FIX_ROW("2b")        \
                     : [lo] "=&r"(lo_), [hi] "=&r"(hi_)                         \
                     : [b] "r"(base), [a] "r"((uint64_t)(a)) : "memory" : slow_); \
        (dst) = (unsigned __int128)lo_ | (unsigned __int128)hi_ << 64;                              \
    } while (0)
#define H2_FLAT_ST_10(v, base, a)                                               \
    do {                                                                        \
        uint64_t lo_ = (uint64_t)(v), hi_ = (uint64_t)((v) >> 64);              \
        __asm__ goto("1: movq %q[lo], (%[b],%[a])\n" H2_FIX_ROW("1b")           \
                     "2: movw %w[hi], 8(%[b],%[a])\n" H2_FIX_ROW("2b")          \
                     : : [lo] "r"(lo_), [hi] "r"(hi_), [b] "r"(base), [a] "r"((uint64_t)(a)) \
                     : "memory" : slow_);                                       \
    } while (0)
#define H2_FLAT_PROBE_R_(base, a)                                               \
    do {                                                                        \
        uint64_t v_;                                                            \
        __asm__ goto("1: movzbl (%[b],%[a]), %k[v]\n" H2_FIX_ROW("1b")          \
                     : [v] "=r"(v_) : [b] "r"(base), [a] "r"((uint64_t)(a)) : "memory" : slow_); \
        (void)v_;                                                               \
    } while (0)
#define H2_FLAT_PROBE_RW_(base, a)                                              \
    do {                                                                        \
        uint64_t v_;                                                            \
        __asm__ goto("1: movzbl (%[b],%[a]), %k[v]\n" H2_FIX_ROW("1b")          \
                     "2: movb %b[v], (%[b],%[a])\n" H2_FIX_ROW("2b")            \
                     : [v] "=&r"(v_) : [b] "r"(base), [a] "r"((uint64_t)(a)) : "memory" : slow_); \
        (void)v_;                                                               \
    } while (0)
#endif
/* A width no site has (a one-bit value): the helper decides. */
#define H2_FLAT_LD_0(dst, base, a) do { (void)(base); goto slow_; } while (0)
#define H2_FLAT_ST_0(v, base, a) do { (void)(v); (void)(base); goto slow_; } while (0)
/* After a store: its page's card and its region's, zero meaning written
 * (section 4.1, "Dirty tracking is card marking"). The fold reports the
 * page after each written page too, so a store that crosses needs no
 * second card. */
#define H2_FLAT_CARD(cards, a)                                                  \
    do {                                                                        \
        (cards)[(a) >> 12] = 0;                                                 \
        (cards)[(int32_t)((a) >> 22) - 1024] = 0;                               \
    } while (0)
#else
#define H2_FIX_BOUNDS 0, 0, 0u
#endif
#if H2_FLAT_
/* A bounded REP MOVS chunk, flat (section 4.1, "Bulk copies"): the paged
 * form's chunking and count. Both spans stay within their current 4 KiB
 * guest page, so each lies in one host page of the window, whose access
 * is the same throughout: a probe of the source's first byte (a load) and
 * of the destination's (a load and a store of the same byte) checks the
 * whole chunk. A fixup on a probe rejects the chunk having changed
 * nothing, and the ordinary iteration runs, with its faults, MMIO and
 * partial progress. Overlap is judged on guest addresses: two guest pages
 * aliasing one physical page are not seen (a propagating copy between
 * such aliases copies as memmove does). `limit` is the next accounting
 * safe point. */
static inline uint32_t h2_fast_movs(H2Mem *f, uint32_t src, uint32_t dst,
                                  uint32_t count, uint32_t width, int reverse,
                                  uint64_t limit) {
    __label__ slow_;
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
    if (f->preempt && (__atomic_load_n(f->preempt, __ATOMIC_RELAXED) & ~H2_IRQ_QUEUED)) n = 1u;
#endif
    uint32_t from = src, to = dst, bytes = n * width;
    if (reverse) { from -= bytes - width; to -= bytes - width; }
    /* Propagating overlap is element-ordered x86 copying, not memmove.
     * In 64 bits: a chunk on the last page ends at 2^32. */
    if ((uint64_t)from < (uint64_t)to + bytes && (uint64_t)to < (uint64_t)from + bytes && from != to &&
        (reverse ? to < from : to > from)) return 0;
    uint8_t *base = f->base;
    H2_FLAT_PROBE_R_(base, from);
    H2_FLAT_PROBE_RW_(base, to);
    memmove(base + to, base + from, bytes);
    H2_FLAT_CARD(f->cards, to);
#if !H2_ACCOUNTING
    uint64_t previous = f->accesses, charged = (uint64_t)n * 2u;
    f->accesses += charged;
    /* Distance to the first matching increment, including unsigned wrap. */
    if (f->budget - previous - 1u < charged)
        __atomic_fetch_or(f->preempt, 1u, __ATOMIC_RELEASE);
#endif
    return n;
slow_:
    return 0;
}
#else
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
    if (f->preempt && (__atomic_load_n(f->preempt, __ATOMIC_RELAXED) & ~H2_IRQ_QUEUED)) n = 1u;
#endif
    H2MemoryShared *m = f->memory;
    uint32_t unlocked = 0;
    if (!__atomic_compare_exchange_n(&m->gate, &unlocked, 1u, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return 0;
    H2MemoryData *d = &m->data;
    if (!d->pages) { H2_GATE_RELEASE(m); return 0; }
    H2FastPage sp = d->pages[src >> 12], dp = d->pages[dst >> 12];
    if (!(sp.permissions & 1u) || !(dp.permissions & 2u) ||
        sp.physical > 0x04000000u - 4096u || dp.physical > 0x04000000u - 4096u) {
        H2_GATE_RELEASE(m); return 0;
    }
    uint32_t from = sp.physical + so, to = dp.physical + doff, bytes = n * width;
    if (reverse) { from -= bytes - width; to -= bytes - width; }
    /* Propagating overlap is element-ordered x86 copying, not memmove.
     * Compare physical spans: virtual aliases must not hide overlap. */
    if (from < to + bytes && to < from + bytes && from != to &&
        (reverse ? to < from : to > from)) {
        H2_GATE_RELEASE(m); return 0;
    }
    memmove(d->ram + to, d->ram + from, bytes);
    if (d->watching & 1u) {
        uint32_t p = dp.physical >> 12, word = p >> 6;
        d->dirty[word] |= UINT64_C(1) << (p & 63u);
        if (word < d->dirty_lo) d->dirty_lo = word;
        if (word + 1u > d->dirty_hi) d->dirty_hi = word + 1u;
    }
    H2_GATE_RELEASE(m);
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
#endif
/* ---- Accesses under a held gate. ---------------------------------------
 * Take a clear gate: the view, or NULL. Once per hold, not per access. */
static inline H2MemoryData *h2_gate_try(H2FastAccess *f) {
    H2MemoryShared *m = f->memory;
    uint32_t unlocked = 0;
    if (!__atomic_compare_exchange_n(&m->gate, &unlocked, H2_GATE_HELD, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return NULL;
    if (!m->data.pages) {
        H2_GATE_RELEASE(m);
        return NULL;
    }
    return &m->data;
}
/* Record a hold taken with view `d`, or none (NULL; `data` is then stale
 * and unused until the next hold). */
static H2_ALWAYS_INLINE void h2_gate_adopt(H2Gate *g, H2MemoryData *d) {
    if (d) {
#if H2_RAM_PAGE_CACHE && H2_FASTMEM
        g->ram_page[0].end = g->ram_page[1].end = 0;
#endif
        g->data = d;
        g->dirtied = UINT32_MAX;
        g->held = 1u;
    } else {
        g->held = 0u;
    }
}
static H2_ALWAYS_INLINE void h2_gate_drop(H2FastAccess *f, H2Gate *g) {
    if (g->held) {
        H2_GATE_RELEASE(f->memory);
        g->held = 0u;
    }
}
/* At a loop header: bound the hold, and give way to a waiting Rust guard. */
static H2_ALWAYS_INLINE void h2_gate_tick(H2FastAccess *f, H2Gate *g) {
    if (g->held && (++g->held > H2_GATE_TRIPS
                    || (__atomic_load_n(&f->memory->gate, __ATOMIC_RELAXED) & H2_GATE_WANTED))) {
        H2_GATE_RELEASE(f->memory);
        g->held = 0u;
    }
}
/* H2_ENTRY_GATE=1 (emit-time, default off): a handler with inline loads
 * or stores takes the gate as it starts (H2_GATE_ENTER in h2gen.h, after
 * its entry calls: acquire and the budget), and again when a direct call
 * returns inline, instead of its first access taking it in h2_access_cold.
 * A busy gate or no view leaves it not held, and that first access is the
 * cold one as before. */
#ifndef H2_ENTRY_GATE
#define H2_ENTRY_GATE 0
#endif
#if H2_ENTRY_GATE && H2_FASTMEM && !H2_FLAT_
#define H2_GATE_ENTER_(f, g)                                                    \
    do {                                                                        \
        if (!(g).held && (f)) h2_gate_adopt(&(g), h2_gate_try(f));              \
    } while (0)
#else
#define H2_GATE_ENTER_(f, g) ((void)0)
#endif
/* H2_VIEW_CHARGE (on in h2 build/emit; counting modules): spend the
 * handler's count in the RAM view, inline, where it called
 * h2abi_v1_charge, h2abi_v1_budget or h2abi_v1_exit (h2gen.h). The view's
 * count is the runtime's to settle, and it settles it before anything that
 * looks at the slice: every Rust helper first, acquire, and the runner after
 * every handler it invokes (call_native). The view's budget is the slice's
 * accesses left when last settled (all ones: none would expire), so the
 * spend that reaches it is the one whose charge would have expired the
 * slice, and raises SLICE on the preempt word as that charge does. */
static H2_ALWAYS_INLINE void h2_view_spend(H2View *f, uint64_t n) {
    uint64_t previous = f->accesses;
    f->accesses = previous + n;
    /* Distance to the first matching increment, including unsigned wrap. */
    if (f->budget - previous - 1u < n) __atomic_fetch_or(f->preempt, 1u, __ATOMIC_RELEASE); /* SLICE */
}
/* What h2abi_v1_budget answers: the accesses left before the slice
 * expires, or all ones when nothing would (unarmed, or already spent). */
static H2_ALWAYS_INLINE uint64_t h2_view_left(const H2View *f) {
    return f->budget == UINT64_MAX || f->accesses >= f->budget ? UINT64_MAX : f->budget - f->accesses;
}
/* Whether physical page `physical` is already marked dirty. A set bit lies
 * inside [dirty_lo, dirty_hi): every marker sets the bit and widens the
 * bounds under the gate, and the drain clears both under it. So a marked
 * page needs nothing more. */
#define H2_DIRTY_MARKED(d, physical) \
    (((d)->dirty[(physical) >> 18] >> (((physical) >> 12) & 63u)) & 1u)
/* H2_DIRTY_INLINE=1 (emit-time, `H2_DIRTY_INLINE` in the build's
 * environment; default off): store sites test the bit themselves and call
 * h2_mark_dirty only for an unmarked page, a load, a shift and a branch
 * more per store site in exchange for the call. */
#ifndef H2_DIRTY_INLINE
#define H2_DIRTY_INLINE 0
#endif
/* Mark a physical page dirty for the Rust side's drain. Out of line and
 * register-preserving: a site calls it only for a store to a page other
 * than the one this hold marked last (H2Gate.dirtied). A page already
 * marked (the usual case: hot pages stay marked until the next drain)
 * writes nothing. */
static H2_COLD H2_PRESERVE void h2_mark_dirty(H2MemoryData *d, uint32_t physical) {
    uint32_t p = physical >> 12, word = p >> 6;
    uint64_t bit = UINT64_C(1) << (p & 63u);
    if (d->dirty[word] & bit) return;
    d->dirty[word] |= bit;
    if (word < d->dirty_lo) d->dirty_lo = word;
    if (word + 1u > d->dirty_hi) d->dirty_hi = word + 1u;
}
/* The inline access path. Under the held gate `g` (an H2Gate lvalue), may
 * `width` bytes at guest `addr` be read (write = 0) or written (write = 1)
 * in place? If so `host` is set to their host address (`page`, a caller's
 * H2FastPage, is scratch); a writable span's physical page has been marked
 * dirty when a watch is active, and the access charged (unless the handler
 * counts accesses itself), all under the gate before the caller accesses,
 * so no Rust guard sees one without the other. If not, it jumps to `slow` having accessed nothing: gate not
 * held, a span crossing its page, no permission, or not RAM.
 *
 * Every emitted site passes a constant width, so the crossing test is one
 * compare and the caller's memcpy of `width` bytes at `host` is a single
 * load or store: no call, no libc, no staging buffer. The tests are plain
 * branches to one label, and the caller declares every local in the
 * label's scope, so that -O1 too compiles each test to a compare and a
 * branch (a jump out of a scope with locals goes through a clang cleanup
 * switch that -O1 leaves in). Everything else is one call to
 * h2_access_cold. */
#define H2_FAST_WIDTH_OK(width) ((width) == 1u || (width) == 2u || (width) == 4u || (width) == 8u || (width) == 10u || (width) == 16u)
#if H2_FASTMEM
#define H2_HELD_TRY(f, g, addr, width, write, page, host, slow)                 \
    do {                                                                        \
        if (!(g).held || !H2_FAST_WIDTH_OK(width) || ((addr) & 4095u) > 4096u - (width)) goto slow; \
        (page) = (g).data->pages[(addr) >> 12];                                 \
        if (!((page).permissions & ((write) ? 2u : 1u))) goto slow;             \
        if ((page).physical > 0x04000000u - 4096u) goto slow;                   \
        if ((write) && ((g).data->watching & 1u) && (page).physical != (g).dirtied) {  \
            if (!H2_DIRTY_INLINE || !H2_DIRTY_MARKED((g).data, (page).physical)) \
                h2_mark_dirty((g).data, (page).physical);                       \
            (g).dirtied = (page).physical;                                      \
        }                                                                       \
        H2_FAST_CHARGE(f);                                                      \
        (host) = (g).data->ram + (page).physical + ((addr) & 4095u);            \
    } while (0)
#else
#define H2_HELD_TRY(f, g, addr, width, write, page, host, slow) do { (void)(g); (void)(page); (host) = NULL; goto slow; } while (0)
#endif
/* Under this gate pages, RAM backing, page permissions and watch state
 * cannot change. A single unsigned range check validates the entire span,
 * including the final 32-bit guest page. An empty slot gives off >= 4096;
 * an address below the cached page underflows in uint64_t. Never form a
 * host pointer until the range check succeeds. Cold helpers and direct
 * callees may have released the gate: adoption clears both slots. */
#if H2_RAM_PAGE_CACHE && H2_FASTMEM
#define H2_PAGED_TRY(f, g, addr, width, write, page, host, slow)                 \
    do {                                                                        \
        H2RamPage *slot_ = &(g).ram_page[(write)];                              \
        uint64_t off_ = (uint64_t)(addr) + 4096u - slot_->end;                  \
        if ((g).held && H2_FAST_WIDTH_OK(width) && off_ <= 4096u - (width)) {   \
            (host) = slot_->base + off_;                                        \
            H2_FAST_CHARGE(f);                                                  \
        } else {                                                                \
            H2_HELD_TRY(f, g, addr, width, write, page, host, slow);             \
            slot_->end = (uint64_t)((addr) | 4095u) + 1u;                       \
            slot_->base = (host) - ((addr) & 4095u);                            \
        }                                                                       \
    } while (0)
#else
#define H2_PAGED_TRY H2_HELD_TRY
#endif
/* The opt-in span image (H2_FLAT_SPAN). The original view's watching
 * field was always 0 or 1. Bit 31 opts into the appended flat descriptor.
 * Never read past the v1 prefix without it, including when running a new
 * module against an older runtime. */
#if H2_FLAT_SPAN && H2_FASTMEM
typedef struct H2FlatView {
    uint8_t *base;
    uint8_t *stores;
    uint32_t plane_mask;
} H2FlatView;
typedef struct H2FlatMemoryData {
    H2MemoryData prefix;
    H2FlatView flat;
    uint8_t *cold_stores;
    const uint64_t *software_pages;
} H2FlatMemoryData;
#if UINTPTR_MAX == UINT64_MAX
_Static_assert(offsetof(H2FlatMemoryData, flat) == 2080 && sizeof(H2FlatView) == 24, "flat memory layout");
_Static_assert(offsetof(H2FlatMemoryData, cold_stores) == 2104 && sizeof(H2FlatMemoryData) == 2120, "flat watch layout");
#endif

static H2_ALWAYS_INLINE uint8_t *h2_flat_span(const H2Gate *g, uint32_t addr,
                                              uint32_t size, uint32_t rights) {
    if (!g->held || !(g->data->watching & UINT32_C(0x80000000))) return NULL;
    const H2FlatView *v = &((const H2FlatMemoryData *)g->data)->flat;
    H2FastPage p = g->data->pages[addr >> 12];
    if ((p.permissions & rights) != rights ||
        !size || size > 4096u - (addr & 4095u)) return NULL;
    if ((rights & 2u) && (g->data->watching & 1u)) {
        /* The mirror is indexed by physical page. It also covers a full
         * permitted page that could not share a guest VA window. */
        if (p.physical & ~UINT32_C(0x03fff000)) return NULL;
        uint8_t *stores = v->stores;
        if (!stores) {
            if (!(g->data->watching & UINT32_C(0x40000000))) return NULL;
            const H2FlatMemoryData *ext = (const H2FlatMemoryData *)g->data;
            uint32_t page = p.physical >> 12;
            if ((ext->software_pages[page >> 6] >> (page & 63u)) & 1u) return NULL;
            stores = ext->cold_stores;
        }
        uint32_t pa = p.physical | (addr & 4095u);
        size_t plane = (pa >> 12) & v->plane_mask;
        return stores + (plane << 26) + pa;
    }
    if (p.permissions & 8u) return v->base + addr;
    uint32_t window = (p.permissions >> 4) & 3u;
    if (!window) return NULL;
    /* Only 16 KiB hosts advertise a shifted window. Each starts one
     * stride after its predecessor and shifts guest VA by a further
     * 4 KiB. A v1 flat runtime never sets the window bits. */
    return v->base + window * (UINT64_C(0x100000000) + 3u * 16384u + 4096u) + addr;
}
/* An image may run with the flat backend disabled, including a private
 * file restore. Probe under the gate and preserve the paged entry policy. */
static H2_ALWAYS_INLINE uint8_t *h2_begin_span(H2FastAccess *f, H2Gate *g,
                                              uint32_t addr, uint32_t size, uint32_t rights) {
    if (!g->held && f) {
        H2MemoryData *d = h2_gate_try(f);
        if (d && !(d->watching & UINT32_C(0x80000000))) {
            H2_GATE_RELEASE(f->memory);
            return NULL;
        }
        h2_gate_adopt(g, d);
    }
    return h2_flat_span(g, addr, size, rights);
}
#define H2_SITE_TRY(f, g, addr, width, write, page, host, slow, on_flat)        \
    do {                                                                        \
        (host) = H2_FAST_WIDTH_OK(width) ?                                      \
            h2_flat_span(&(g), (addr), (width), (write) ? 2u : 1u) : NULL;       \
        if (host) { on_flat; H2_FAST_CHARGE(f); }                               \
        else { H2_PAGED_TRY(f, g, addr, width, write, page, host, slow); }       \
    } while (0)
#else
#define H2_SITE_TRY(f, g, addr, width, write, page, host, slow, on_flat)        \
    H2_PAGED_TRY(f, g, addr, width, write, page, host, slow)
#endif
/* What h2_access_cold leaves: the view if the handler now holds the gate
 * (else NULL), and the ordinary helper's status (0: it accessed). */
typedef struct H2Cold {
    H2MemoryData *held;
    uint32_t status;
#ifdef H2_FLAT_STATS_OWNER
    uint32_t flat;
#endif
} H2Cold;
/* Everything an inline access does not do, out of line, cold and
 * register-preserving, so a site is the inline path plus one call. With the
 * gate held (`held`), the inline check failed: drop the gate and take the
 * ordinary helper. Without it, take the gate and retry the inline check,
 * accessing `buf` under it and keeping the gate for the accesses after;
 * failing that, drop it and take the helper. The helper path is exactly
 * the old slow access: fault order, MMIO and whole-span failure are the
 * helper's. A load's bytes land in `buf`; a store's come from it. */
/* `width` bytes (one of H2_FAST_WIDTH_OK's) from `src` to `dst`, each as a
 * constant-size copy: in h2_access_cold the width is not a constant, and a
 * libc memcpy there was a call, with every register preserve_most keeps
 * saved around it, at the first access of nearly every guest function. */
static H2_ALWAYS_INLINE void h2_copy_width(uint8_t *dst, const uint8_t *src, uint32_t width) {
    switch (width) {
    case 1u: memcpy(dst, src, 1u); break;
    case 2u: memcpy(dst, src, 2u); break;
    case 4u: memcpy(dst, src, 4u); break;
    case 8u: memcpy(dst, src, 8u); break;
    case 10u: memcpy(dst, src, 10u); break;
    case 16u: memcpy(dst, src, 16u); break;
    default: memcpy(dst, src, width); break;
    }
}
#if !H2_FLAT_
static H2_COLD H2_PRESERVE H2Cold h2_access_cold(ExecCtxOpaque *ctx, H2FastAccess *f, uint32_t held, uint32_t pc,
                                                 uint32_t addr, uint32_t width, int write, uint8_t *buf) {
    H2Cold r = {0};
#if H2_FASTMEM
    if (f && held) {
        H2_GATE_RELEASE(f->memory);
    } else if (f) {
        H2Gate g = {.dirtied = UINT32_MAX};
        h2_gate_adopt(&g, h2_gate_try(f));
        H2FastPage page;
        uint8_t *host;
#if H2_FLAT_SPAN
        H2_SITE_TRY(f, g, addr, width, write, page, host, helper, H2_COLD_FLAT(r));
#else
        H2_HELD_TRY(f, g, addr, width, write, page, host, helper);
#endif
        if (write) h2_copy_width(host, buf, width);
        else h2_copy_width(buf, host, width);
        r.held = g.data;
        return r;
    helper:
        h2_gate_drop(f, &g);
    }
#else
    (void)f; (void)held;
#endif
    r.status = write ? H2_MEM_WRITE_SLOW(ctx, pc, addr, width, 1u, buf) : H2_MEM_READ_SLOW(ctx, pc, addr, width, 1u, buf);
    return r;
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
#else
/* The flat model has no gate: what the shared macros hold, drop and tick
 * is nothing (h2gen.h's H2_GATE_DROP, the emitter's REP loop tick). */
#undef h2_gate_drop
#define h2_gate_drop(f, g) ((void)(f), (void)(g))
#define h2_gate_tick(f, g) ((void)(f), (void)(g))
#endif
#endif
