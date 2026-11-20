/* Contract change slice F1: one inline load and one inline store per width,
 * each an `asm goto` whose access instruction has a row in the fixup table
 * (docs/runtime/contract-thin-transfers-flat-ram.md, section 4.1). A host
 * fault on the instruction resumes at the site's `slow` label, which here
 * only says so: the generated form calls the checked helper there.
 *
 * H2_FIX_ABS=0 emits rows as two 32-bit self-relative words
 * (hostfault::Format::Relative32); H2_FIX_ABS=1 as two 64-bit absolute
 * addresses (Format::Absolute64), the fallback for an object format that
 * cannot subtract across sections. Freestanding: no libc headers, so it
 * compiles with only `-target` for every target emit knows.
 * Synthetic: no guest data. */
#include <stdint.h>

#ifndef H2_FIX_ABS
#define H2_FIX_ABS 0
#endif

#if defined(__APPLE__)
#define H2FIX_PUSH ".pushsection __DATA_CONST,__h2_fix,regular\n"
#elif defined(_WIN32)
#define H2FIX_PUSH ".pushsection .h2_fix$m,\"dr\"\n"
#elif H2_FIX_ABS
/* Absolute rows need load-time relocation: a writable section, or the
 * linker refuses a text relocation in a shared object. */
#define H2FIX_PUSH ".pushsection h2_fix,\"aw\"\n"
#else
/* A C identifier, so the linker defines __start_h2_fix and __stop_h2_fix. */
#define H2FIX_PUSH ".pushsection h2_fix,\"a\"\n"
#endif

#if H2_FIX_ABS
#define H2FIX_ROW(insn)                                                         \
    H2FIX_PUSH ".p2align 3\n.quad " insn "\n.quad %l[slow]\n.popsection\n"
#else
#define H2FIX_ROW(insn)                                                         \
    H2FIX_PUSH ".p2align 2\n.long " insn " - .\n.long %l[slow] - .\n.popsection\n"
#endif

#if defined(_WIN32)
#define H2FIX_EXPORT __declspec(dllexport)
#else
#define H2FIX_EXPORT __attribute__((visibility("default")))
#endif
#define H2FIX_FN H2FIX_EXPORT __attribute__((noinline))

typedef unsigned char h2fix_v16 __attribute__((vector_size(16)));

/* ---- The table's bounds and encoding. ---------------------------------- */
#if defined(__APPLE__)
extern const char h2fix_start_[] __asm("section$start$__DATA_CONST$__h2_fix");
extern const char h2fix_end_[] __asm("section$end$__DATA_CONST$__h2_fix");
#elif defined(_WIN32)
/* The linker orders `.h2_fix$a`, `$m`, `$z` by suffix. The markers are
 * one row each and are skipped. */
#pragma section(".h2_fix$a", read)
#pragma section(".h2_fix$z", read)
__declspec(allocate(".h2_fix$a")) const uint64_t h2fix_first_[2] = {0, 0};
__declspec(allocate(".h2_fix$z")) const uint64_t h2fix_last_[2] = {0, 0};
#define h2fix_start_ ((const char *)h2fix_first_ + (H2_FIX_ABS ? 16 : 8))
#define h2fix_end_ ((const char *)h2fix_last_)
#else
extern const char __start_h2_fix[] __attribute__((visibility("hidden")));
extern const char __stop_h2_fix[] __attribute__((visibility("hidden")));
#define h2fix_start_ __start_h2_fix
#define h2fix_end_ __stop_h2_fix
#endif

H2FIX_FN uint32_t h2fix_format(void) { return H2_FIX_ABS; }
H2FIX_FN void h2fix_bounds(const void **start, const void **end) {
    *start = h2fix_start_;
    *end = h2fix_end_;
}

/* What a landing returns in place of the helper's value. */
#define H2FIX_SLOW 0x5a5a5a5a5a5a5a5aull

/* ---- One access per width. -------------------------------------------- */
#if defined(__aarch64__)

#define H2FIX_LD(name, insn, type)                                              \
    H2FIX_FN uint64_t name(const uint8_t *base, uint32_t addr, uint32_t *slow) { \
        type v;                                                                 \
        __asm__ goto("1: " insn " %w[v], [%[b], %w[a], uxtw]\n" H2FIX_ROW("1b") \
                     : [v] "=r"(v)                                              \
                     : [b] "r"(base), [a] "r"(addr)                             \
                     : "memory"                                                 \
                     : slow);                                                   \
        return v;                                                               \
    slow:                                                                       \
        *slow = 1;                                                              \
        return H2FIX_SLOW;                                                      \
    }
#define H2FIX_ST(name, insn)                                                    \
    H2FIX_FN void name(uint8_t *base, uint32_t addr, uint64_t v, uint32_t *slow) { \
        __asm__ goto("1: " insn " %w[v], [%[b], %w[a], uxtw]\n" H2FIX_ROW("1b") \
                     :                                                          \
                     : [v] "r"((uint32_t)v), [b] "r"(base), [a] "r"(addr)       \
                     : "memory"                                                 \
                     : slow);                                                   \
        return;                                                                 \
    slow:                                                                       \
        *slow = 1;                                                              \
    }
H2FIX_LD(h2fix_ld1, "ldrb", uint32_t)
H2FIX_LD(h2fix_ld2, "ldrh", uint32_t)
H2FIX_LD(h2fix_ld4, "ldr", uint32_t)
H2FIX_ST(h2fix_st1, "strb")
H2FIX_ST(h2fix_st2, "strh")
H2FIX_ST(h2fix_st4, "str")

H2FIX_FN uint64_t h2fix_ld8(const uint8_t *base, uint32_t addr, uint32_t *slow) {
    uint64_t v;
    __asm__ goto("1: ldr %x[v], [%[b], %w[a], uxtw]\n" H2FIX_ROW("1b")
                 : [v] "=r"(v) : [b] "r"(base), [a] "r"(addr) : "memory" : slow);
    return v;
slow:
    *slow = 1;
    return H2FIX_SLOW;
}
H2FIX_FN void h2fix_st8(uint8_t *base, uint32_t addr, uint64_t v, uint32_t *slow) {
    __asm__ goto("1: str %x[v], [%[b], %w[a], uxtw]\n" H2FIX_ROW("1b")
                 : : [v] "r"(v), [b] "r"(base), [a] "r"(addr) : "memory" : slow);
    return;
slow:
    *slow = 1;
}

H2FIX_FN void h2fix_ld16(const uint8_t *base, uint32_t addr, uint8_t *out, uint32_t *slow) {
    h2fix_v16 v;
    __asm__ goto("1: ldr %q[v], [%[b], %w[a], uxtw]\n" H2FIX_ROW("1b")
                 : [v] "=w"(v) : [b] "r"(base), [a] "r"(addr) : "memory" : slow);
    for (int i = 0; i < 16; i++) out[i] = v[i];
    return;
slow:
    *slow = 1;
}
H2FIX_FN void h2fix_st16(uint8_t *base, uint32_t addr, const uint8_t *in, uint32_t *slow) {
    h2fix_v16 v;
    for (int i = 0; i < 16; i++) v[i] = in[i];
    __asm__ goto("1: str %q[v], [%[b], %w[a], uxtw]\n" H2FIX_ROW("1b")
                 : : [v] "w"(v), [b] "r"(base), [a] "r"(addr) : "memory" : slow);
    return;
slow:
    *slow = 1;
}

/* Ten bytes: two instructions, two rows, one label. */
H2FIX_FN void h2fix_ld10(const uint8_t *base, uint32_t addr, uint8_t *out, uint32_t *slow) {
    uint64_t lo;
    uint32_t hi;
    const uint8_t *t;
    __asm__ goto("add %[t], %[b], %w[a], uxtw\n"
                 "1: ldr %x[lo], [%[t]]\n" H2FIX_ROW("1b")
                 "2: ldrh %w[hi], [%[t], #8]\n" H2FIX_ROW("2b")
                 : [lo] "=&r"(lo), [hi] "=&r"(hi), [t] "=&r"(t)
                 : [b] "r"(base), [a] "r"(addr) : "memory" : slow);
    for (int i = 0; i < 8; i++) out[i] = (uint8_t)(lo >> (8 * i));
    out[8] = (uint8_t)hi;
    out[9] = (uint8_t)(hi >> 8);
    return;
slow:
    *slow = 1;
}
H2FIX_FN void h2fix_st10(uint8_t *base, uint32_t addr, const uint8_t *in, uint32_t *slow) {
    uint64_t lo = 0;
    for (int i = 0; i < 8; i++) lo |= (uint64_t)in[i] << (8 * i);
    uint32_t hi = (uint32_t)in[8] | (uint32_t)in[9] << 8;
    uint8_t *t;
    __asm__ goto("add %[t], %[b], %w[a], uxtw\n"
                 "1: str %x[lo], [%[t]]\n" H2FIX_ROW("1b")
                 "2: strh %w[hi], [%[t], #8]\n" H2FIX_ROW("2b")
                 : [t] "=&r"(t)
                 : [lo] "r"(lo), [hi] "r"(hi), [b] "r"(base), [a] "r"(addr)
                 : "memory" : slow);
    return;
slow:
    *slow = 1;
}

#elif defined(__x86_64__)

typedef long long h2fix_x16 __attribute__((vector_size(16)));

#define H2FIX_LD(name, insn, reg)                                               \
    H2FIX_FN uint64_t name(const uint8_t *base, uint32_t addr, uint32_t *slow) { \
        uint64_t v, a = addr;                                                   \
        __asm__ goto("1: " insn " (%[b],%[a]), %" reg "[v]\n" H2FIX_ROW("1b")  \
                     : [v] "=r"(v)                                              \
                     : [b] "r"(base), [a] "r"(a)                                \
                     : "memory"                                                 \
                     : slow);                                                   \
        return v;                                                               \
    slow:                                                                       \
        *slow = 1;                                                              \
        return H2FIX_SLOW;                                                      \
    }
#define H2FIX_ST(name, insn, reg)                                               \
    H2FIX_FN void name(uint8_t *base, uint32_t addr, uint64_t v, uint32_t *slow) { \
        uint64_t a = addr;                                                      \
        __asm__ goto("1: " insn " %" reg "[v], (%[b],%[a])\n" H2FIX_ROW("1b")  \
                     :                                                          \
                     : [v] "r"(v), [b] "r"(base), [a] "r"(a)                    \
                     : "memory"                                                 \
                     : slow);                                                   \
        return;                                                                 \
    slow:                                                                       \
        *slow = 1;                                                              \
    }
/* A 32-bit destination zero-extends into the whole register. */
H2FIX_LD(h2fix_ld1, "movzbl", "k")
H2FIX_LD(h2fix_ld2, "movzwl", "k")
H2FIX_LD(h2fix_ld4, "movl", "k")
H2FIX_LD(h2fix_ld8, "movq", "q")
H2FIX_ST(h2fix_st1, "movb", "b")
H2FIX_ST(h2fix_st2, "movw", "w")
H2FIX_ST(h2fix_st4, "movl", "k")
H2FIX_ST(h2fix_st8, "movq", "q")

H2FIX_FN void h2fix_ld16(const uint8_t *base, uint32_t addr, uint8_t *out, uint32_t *slow) {
    h2fix_x16 v;
    uint64_t a = addr;
    __asm__ goto("1: movdqu (%[b],%[a]), %[v]\n" H2FIX_ROW("1b")
                 : [v] "=x"(v) : [b] "r"(base), [a] "r"(a) : "memory" : slow);
    for (int i = 0; i < 16; i++) out[i] = (uint8_t)((uint64_t)v[i / 8] >> (8 * (i % 8)));
    return;
slow:
    *slow = 1;
}
H2FIX_FN void h2fix_st16(uint8_t *base, uint32_t addr, const uint8_t *in, uint32_t *slow) {
    h2fix_x16 v = {0, 0};
    for (int i = 0; i < 16; i++) v[i / 8] |= (long long)((uint64_t)in[i] << (8 * (i % 8)));
    uint64_t a = addr;
    __asm__ goto("1: movdqu %[v], (%[b],%[a])\n" H2FIX_ROW("1b")
                 : : [v] "x"(v), [b] "r"(base), [a] "r"(a) : "memory" : slow);
    return;
slow:
    *slow = 1;
}

/* Ten bytes: two instructions, two rows, one label. */
H2FIX_FN void h2fix_ld10(const uint8_t *base, uint32_t addr, uint8_t *out, uint32_t *slow) {
    uint64_t lo, hi, a = addr;
    __asm__ goto("1: movq (%[b],%[a]), %q[lo]\n" H2FIX_ROW("1b")
                 "2: movzwl 8(%[b],%[a]), %k[hi]\n" H2FIX_ROW("2b")
                 : [lo] "=&r"(lo), [hi] "=&r"(hi)
                 : [b] "r"(base), [a] "r"(a) : "memory" : slow);
    for (int i = 0; i < 8; i++) out[i] = (uint8_t)(lo >> (8 * i));
    out[8] = (uint8_t)hi;
    out[9] = (uint8_t)(hi >> 8);
    return;
slow:
    *slow = 1;
}
H2FIX_FN void h2fix_st10(uint8_t *base, uint32_t addr, const uint8_t *in, uint32_t *slow) {
    uint64_t lo = 0, a = addr;
    for (int i = 0; i < 8; i++) lo |= (uint64_t)in[i] << (8 * i);
    uint64_t hi = (uint64_t)in[8] | (uint64_t)in[9] << 8;
    __asm__ goto("1: movq %q[lo], (%[b],%[a])\n" H2FIX_ROW("1b")
                 "2: movw %w[hi], 8(%[b],%[a])\n" H2FIX_ROW("2b")
                 : : [lo] "r"(lo), [hi] "r"(hi), [b] "r"(base), [a] "r"(a)
                 : "memory" : slow);
    return;
slow:
    *slow = 1;
}

#else
#error "fixup fixture: arm64 or x86-64 only"
#endif
