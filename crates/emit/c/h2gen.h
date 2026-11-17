/* Support header for generated guest code. Written next to every generated
 * module by crates/emit; not derived from any disc. Built on h2abi.h (the
 * runtime's versioned C ABI).
 *
 * Every helper here is total and free of undefined behaviour: unsigned
 * fixed-width arithmetic only, shifts checked against the width, division
 * by zero defined (the IR raises #DE explicitly before dividing), signed
 * operations done on unsigned bit patterns. Compile with -fno-strict-aliasing
 * and without fast-math. */
#ifndef H2GEN_H
#define H2GEN_H
#include "h2abi.h"
#include <string.h>
#include "h2fastmem.h"

typedef unsigned __int128 h2u128;
/* SSE floating-point reference semantics (h2s_*), written next to this
 * header by crates/emit. */
#include "h2simd.h"
#include "h2sse_native.h"
/* x87 helpers (h2x87_v1_*), implemented by crates/x87 and linked from the
 * runtime; the header is written next to this one by crates/emit. */
#include "h2x87.h"
#include "h2x87_native.h"

/* ---- Emitter shims: things the v1 ABI does not provide yet. -------------
 * Named h2shim_* so they are easy to find and replace once the runtime grows
 * an equivalent. */

/* Non-memory guest exceptions and stops. Each records a context event and
 * returns its id, for a FAULT or STOP transfer. `pc` is the instruction;
 * `saved_eip` is the EIP the exception reports: `pc` for faults, the next
 * instruction for traps (int3, int n, into). */
#define H2SHIM_DIVIDE_ERROR 1u
#define H2SHIM_BREAKPOINT 2u        /* trap */
#define H2SHIM_INVALID_OPCODE 3u
#define H2SHIM_INTERRUPT 4u         /* trap; arg: vector */
#define H2SHIM_OVERFLOW 5u          /* trap */
#define H2SHIM_BOUND_RANGE 6u
#define H2SHIM_GENERAL_PROTECTION 7u
#define H2SHIM_HALT 8u              /* stop */
#define H2SHIM_BAD_ENTRY 9u         /* stop; arg: the start selector given */
#define H2SHIM_UNKNOWN_TARGET 10u   /* stop; arg: target */
#define H2SHIM_DESCENT_ERROR 11u    /* stop: the CFG could not follow control past pc */
#define H2SHIM_UNMODELLED 12u       /* stop: popfd changing TF/IF/IOPL/NT */
#define H2SHIM_SIMD_EXCEPTION 13u   /* #XM: unmasked SSE FP exception (fault) */
#define H2SHIM_FP_ERROR 14u         /* #MF: pending unmasked x87 exception (fault); arg: status word */
uint32_t h2shim_v1_event(ExecCtxOpaque *, uint32_t pc, uint32_t saved_eip, uint32_t code, uint32_t arg);
/* An instruction the lifter does not implement: a STOP with diagnostics. */
uint32_t h2shim_v1_unsupported(ExecCtxOpaque *, uint32_t pc, const char *mnemonic, const char *detail);
/* rdtsc at pc: the guest's time-stamp counter, which the runtime keeps. */
uint64_t h2shim_v1_rdtsc(ExecCtxOpaque *, uint32_t pc);
/* Atomics use the ABI's own operation-wide accessors (h2abi_v1_atomic_rmw,
 * h2abi_v1_xchg, h2abi_v1_cmpxchg); see H2_ATOMIC and
 * crates/emit/RUNTIME-CONTRACT.md. */

/* Registration tables. kind: 0 = function entry, 1 = address-taken interior
 * entry of the same owner (its own selector). abi_version is the version
 * probe of the object that defines fn (one per generated file, via
 * H2_DEFINE_GENERATED_ABI_NAMED), for register_generated_object. */
typedef struct H2GenEntry {
    uint32_t addr, kind;
    H2Handler fn;
    uint32_t (*abi_version)(void);
} H2GenEntry;
/* A validated continuation PC: resume `owner`'s handler with `start`. */
typedef struct H2GenResume {
    uint32_t pc, owner, start;
} H2GenResume;
/* features: H2GEN_FEATURE_ACCOUNTING or 0 (runtime generated.rs). */
#define H2GEN_FEATURE_ACCOUNTING 1u
typedef struct H2GenModule {
    uint32_t abi_version, entry_count, resume_count, features;
    const H2GenEntry *entries;
    const H2GenResume *resumes;
} H2GenModule;
extern const H2GenModule h2gen_module;

/* ---- Strict integer helpers. ------------------------------------------- */

#define H2_INT_HELPERS(N, T, U, M)                                              \
    static inline T h2_shl##N(T a, T c) { return c >= N ? (T)0 : (T)((U)a << c); } \
    static inline T h2_shr##N(T a, T c) { return c >= N ? (T)0 : (T)((U)a >> c); } \
    static inline T h2_sar##N(T a, T c) {                                       \
        U m = (U)(M), x = (U)a, k = c >= N ? (U)(N - 1) : (U)c;                 \
        return (T)((x >> (N - 1)) ? (~((~x & m) >> k) & m) : (x >> k));         \
    }                                                                           \
    static inline T h2_rol##N(T a, T c) {                                       \
        unsigned k = (unsigned)(c % N);                                         \
        return k ? (T)(((U)a << k) | ((U)a >> (N - k))) : a;                    \
    }                                                                           \
    static inline T h2_ror##N(T a, T c) {                                       \
        unsigned k = (unsigned)(c % N);                                         \
        return k ? (T)(((U)a >> k) | ((U)a << (N - k))) : a;                    \
    }                                                                           \
    static inline T h2_udiv##N(T a, T b) { return b ? (T)((U)a / (U)b) : (T)0; } \
    static inline T h2_urem##N(T a, T b) { return b ? (T)((U)a % (U)b) : a; }   \
    static inline T h2_sdiv##N(T a, T b) {                                      \
        U m = (U)(M), sa = (U)a >> (N - 1), sb = (U)b >> (N - 1);               \
        U ma = sa ? (((U)0 - (U)a) & m) : (U)a, mb = sb ? (((U)0 - (U)b) & m) : (U)b; \
        if (!b) return (T)0;                                                    \
        U q = ma / mb;                                                          \
        return (T)((sa ^ sb) ? (((U)0 - q) & m) : q);                           \
    }                                                                           \
    static inline T h2_srem##N(T a, T b) {                                      \
        U m = (U)(M), sa = (U)a >> (N - 1), sb = (U)b >> (N - 1);               \
        U ma = sa ? (((U)0 - (U)a) & m) : (U)a, mb = sb ? (((U)0 - (U)b) & m) : (U)b; \
        if (!b) return a;                                                       \
        U r = ma % mb;                                                          \
        return (T)(sa ? (((U)0 - r) & m) : r);                                  \
    }

H2_INT_HELPERS(8, uint8_t, uint32_t, 0xffu)
H2_INT_HELPERS(16, uint16_t, uint32_t, 0xffffu)
H2_INT_HELPERS(32, uint32_t, uint32_t, 0xffffffffu)
H2_INT_HELPERS(64, uint64_t, uint64_t, ~(uint64_t)0)
H2_INT_HELPERS(128, h2u128, h2u128, ~(h2u128)0)

/* PF: even parity of the low byte. */
static inline uint8_t h2_parity(uint8_t v) { return (uint8_t)!__builtin_parity(v); }

static inline uint8_t h2_ctz8(uint8_t a) { return a ? (uint8_t)__builtin_ctz(a) : 8; }
static inline uint16_t h2_ctz16(uint16_t a) { return a ? (uint16_t)__builtin_ctz(a) : 16; }
static inline uint32_t h2_ctz32(uint32_t a) { return a ? (uint32_t)__builtin_ctz(a) : 32; }
static inline uint64_t h2_ctz64(uint64_t a) { return a ? (uint64_t)__builtin_ctzll(a) : 64; }
static inline h2u128 h2_ctz128(h2u128 a) {
    uint64_t lo = (uint64_t)a, hi = (uint64_t)(a >> 64);
    return lo ? (h2u128)__builtin_ctzll(lo) : hi ? (h2u128)(64 + __builtin_ctzll(hi)) : 128;
}
static inline uint8_t h2_clz8(uint8_t a) { return a ? (uint8_t)(__builtin_clz(a) - 24) : 8; }
static inline uint16_t h2_clz16(uint16_t a) { return a ? (uint16_t)(__builtin_clz(a) - 16) : 16; }
static inline uint32_t h2_clz32(uint32_t a) { return a ? (uint32_t)__builtin_clz(a) : 32; }
static inline uint64_t h2_clz64(uint64_t a) { return a ? (uint64_t)__builtin_clzll(a) : 64; }
static inline h2u128 h2_clz128(h2u128 a) {
    uint64_t lo = (uint64_t)a, hi = (uint64_t)(a >> 64);
    return hi ? (h2u128)__builtin_clzll(hi) : lo ? (h2u128)(64 + __builtin_clzll(lo)) : 128;
}
static inline uint8_t h2_bswap8(uint8_t a) { return a; }
static inline uint16_t h2_bswap16(uint16_t a) { return __builtin_bswap16(a); }
static inline uint32_t h2_bswap32(uint32_t a) { return __builtin_bswap32(a); }
static inline uint64_t h2_bswap64(uint64_t a) { return __builtin_bswap64(a); }
static inline h2u128 h2_bswap128(h2u128 a) {
    return ((h2u128)__builtin_bswap64((uint64_t)a) << 64) | __builtin_bswap64((uint64_t)(a >> 64));
}

/* ---- Intrinsics (ir::Intrinsic): FP state operations. Each mirrors
 * ir::interp::intrinsic exactly. Arguments, then result pointers; returns 0
 * or the H2SHIM_* fault code to raise, whose event arg is
 * H2_INTRINSIC_ARG. ------------------------------------------------------- */

/* The event arg for an intrinsic's fault code: the x87 status word for #MF
 * (read after the intrinsic, which left the state untouched), else 0. */
#define H2_INTRINSIC_ARG(code) ((code) == H2SHIM_FP_ERROR ? (uint32_t)cpu->x87.status : 0u)
/* An unmasked x87 exception is pending (ES): the next waiting instruction
 * raises #MF instead of running. */
static inline int h2_x87_pending(const CpuAbi *cpu) { return (cpu->x87.status & 0x0080u) != 0; }

/* fnstcw/fnstsw are plain field reads (TOP is in the status word); every
 * other x87 operation is an h2x87_v1_* call, see the H2X87 macros. */
static inline uint32_t h2i_x87_read_control(CpuAbi *cpu, uint16_t *out) { *out = cpu->x87.control; return 0; }
static inline uint32_t h2i_x87_read_status(CpuAbi *cpu, uint16_t *out) { *out = cpu->x87.status; return 0; }
static inline uint32_t h2i_mxcsr_read(CpuAbi *cpu, uint32_t *out) { *out = cpu->mxcsr; return 0; }
/* The guest's Pentium III implements MXCSR bits 0-15 except DAZ (bit 6);
 * ir::interp::MXCSR_MASK. */
#define H2_MXCSR_MASK 0x0000ffbfu
static inline uint32_t h2i_mxcsr_write(CpuAbi *cpu, uint32_t v) {
    if (v & ~H2_MXCSR_MASK) return H2SHIM_GENERAL_PROTECTION;
    cpu->mxcsr = v;
    return 0;
}

static inline h2u128 h2_rx(const CpuAbi *c, unsigned n) {
    h2u128 v;
    memcpy(&v, c->xmm[n], 16);
    return v;
}
static inline void h2_wx(CpuAbi *c, unsigned n, h2u128 v) { memcpy(c->xmm[n], &v, 16); }

/* MMX registers alias the x87 significands in physical order: mmN is
 * st[N] bytes 0-7; a write sets bytes 8-9 (sign, exponent) to 0xffff. */
static inline uint64_t h2_rmm(const CpuAbi *c, unsigned n) {
    uint64_t v;
    memcpy(&v, c->x87.st[n], 8);
    return v;
}
static inline void h2_wmm(CpuAbi *c, unsigned n, uint64_t v) {
    memcpy(c->x87.st[n], &v, 8);
    c->x87.st[n][8] = 0xff;
    c->x87.st[n][9] = 0xff;
}
/* A pending x87 exception stops every MMX instruction and emms with #MF
 * before it does anything, as on hardware: the lifter puts h2i_x87_wait
 * first in each MMX instruction (h2i_mmx_enter runs last, after the
 * instruction's faulting accesses), and h2i_emms checks for itself.
 * h2i_x87_wait is also fwait's lowering. */
static inline uint32_t h2i_x87_wait(CpuAbi *cpu) { return h2_x87_pending(cpu) ? H2SHIM_FP_ERROR : 0u; }
/* Every MMX instruction: TOP = 0, all tags valid. emms: all tags empty. */
static inline uint32_t h2i_mmx_enter(CpuAbi *cpu) { cpu->x87.status &= (uint16_t)~0x3800u; cpu->x87.tag = 0; return 0; }
static inline uint32_t h2i_emms(CpuAbi *cpu) {
    if (h2_x87_pending(cpu)) return H2SHIM_FP_ERROR;
    cpu->x87.tag = 0xffff;
    return 0;
}

/* SSE floating point (ir::Intrinsic::Sse): MXCSR in CpuAbi, h2s_* from
 * h2simd.h returning 1 on #XM. Results written only on success. */
#define H2_SSE_RES(r) ((r) ? H2SHIM_SIMD_EXCEPTION : 0u)
#define H2_SSE_SS(op, impl)                                                           \
    static inline uint32_t h2i_sse_##op(CpuAbi *cpu, h2u128 a, uint32_t b, h2u128 *out) { \
        return H2_SSE_RES(impl(&cpu->mxcsr, a, b, out));                   \
    }
#define H2_SSE_PS(op, impl)                                                           \
    static inline uint32_t h2i_sse_##op(CpuAbi *cpu, h2u128 a, h2u128 b, h2u128 *out) { \
        return H2_SSE_RES(impl(&cpu->mxcsr, a, b, out));                   \
    }
H2_SSE_SS(addss, h2sn_addss) H2_SSE_PS(addps, h2sn_addps) H2_SSE_SS(subss, h2sn_subss) H2_SSE_PS(subps, h2sn_subps)
H2_SSE_SS(mulss, h2sn_mulss) H2_SSE_PS(mulps, h2sn_mulps) H2_SSE_SS(divss, h2sn_divss) H2_SSE_PS(divps, h2sn_divps)
H2_SSE_SS(minss, h2s_minss) H2_SSE_PS(minps, h2s_minps) H2_SSE_SS(maxss, h2s_maxss) H2_SSE_PS(maxps, h2s_maxps)
H2_SSE_SS(sqrtss, h2sn_sqrtss) H2_SSE_SS(cvtsi2ss, h2s_cvtsi2ss)
static inline uint32_t h2i_sse_sqrtps(CpuAbi *cpu, h2u128 b, h2u128 *out) {
    return H2_SSE_RES(h2sn_sqrtps(&cpu->mxcsr, b, out));
}
static inline uint32_t h2i_sse_rcpss(CpuAbi *cpu, h2u128 a, uint32_t b, h2u128 *out) { (void)cpu; *out = h2s_rcpss(a, b); return 0; }
static inline uint32_t h2i_sse_rsqrtss(CpuAbi *cpu, h2u128 a, uint32_t b, h2u128 *out) { (void)cpu; *out = h2s_rsqrtss(a, b); return 0; }
static inline uint32_t h2i_sse_rcpps(CpuAbi *cpu, h2u128 b, h2u128 *out) { (void)cpu; *out = h2s_rcpps(b); return 0; }
static inline uint32_t h2i_sse_rsqrtps(CpuAbi *cpu, h2u128 b, h2u128 *out) { (void)cpu; *out = h2s_rsqrtps(b); return 0; }
static inline uint32_t h2i_sse_cmpss(CpuAbi *cpu, h2u128 a, uint32_t b, uint8_t pred, h2u128 *out) {
    return H2_SSE_RES(h2s_cmpss(&cpu->mxcsr, a, b, pred, out));
}
static inline uint32_t h2i_sse_cmpps(CpuAbi *cpu, h2u128 a, h2u128 b, uint8_t pred, h2u128 *out) {
    return H2_SSE_RES(h2s_cmpps(&cpu->mxcsr, a, b, pred, out));
}
static inline uint32_t h2i_sse_comiss(CpuAbi *cpu, uint32_t a, uint32_t b, uint32_t *out) {
    return H2_SSE_RES(h2s_comiss(&cpu->mxcsr, a, b, out));
}
static inline uint32_t h2i_sse_ucomiss(CpuAbi *cpu, uint32_t a, uint32_t b, uint32_t *out) {
    return H2_SSE_RES(h2s_ucomiss(&cpu->mxcsr, a, b, out));
}
static inline uint32_t h2i_sse_cvtss2si(CpuAbi *cpu, uint32_t a, uint32_t *out) {
    return H2_SSE_RES(h2s_cvtss2si(&cpu->mxcsr, a, out));
}
static inline uint32_t h2i_sse_cvttss2si(CpuAbi *cpu, uint32_t a, uint32_t *out) {
    return H2_SSE_RES(h2s_cvttss2si(&cpu->mxcsr, a, out));
}
static inline uint32_t h2i_sse_cvtpi2ps(CpuAbi *cpu, h2u128 a, uint64_t mm, h2u128 *out) {
    return H2_SSE_RES(h2s_cvtpi2ps(&cpu->mxcsr, a, mm, out));
}
static inline uint32_t h2i_sse_cvtps2pi(CpuAbi *cpu, uint64_t a, uint64_t *out) {
    return H2_SSE_RES(h2s_cvtps2pi(&cpu->mxcsr, a, out));
}
static inline uint32_t h2i_sse_cvttps2pi(CpuAbi *cpu, uint64_t a, uint64_t *out) {
    return H2_SSE_RES(h2s_cvttps2pi(&cpu->mxcsr, a, out));
}

/* ---- Access accounting (module feature H2GEN_FEATURE_ACCOUNTING). -------
 * A module emitted with accounting defines H2_ACCOUNTING to 1 before
 * including this header. Its handlers count guest memory accesses in a
 * local, h2acc, one before each attempted access (the faulting one
 * included). The accesses themselves charge nothing: h2fastmem.h's inline
 * RAM path skips its own count and its slow path takes the uncharged
 * h2abi_v1_read/write_counted. Handlers settle the count on every way
 * out (the shared epilogue's h2abi_v1_exit, H2_EXIT_LATE_) and with
 * h2abi_v1_charge before anything that spends or observes the slice
 * (H2_ACC_SETTLE).
 * The local never escapes, so clang merges the increments along a path.
 * Loops compare it with the budget read on entry (h2left, from
 * h2abi_v1_budget) where they poll, so they leave at the safe point where
 * the charged accessors' slice bit would have made them leave. Without
 * accounting every macro below is the plain, charging form. */
#ifndef H2_ACCOUNTING
#define H2_ACCOUNTING 0
#endif
#if H2_ACCOUNTING
#define H2_ACC_LOCALS() uint64_t h2acc = 0
#define H2_ACC_ONE() (h2acc += 1u)
#define H2_ACC_FLUSH()                                                          \
    do {                                                                        \
        if (h2acc) {                                                            \
            H2_GATE_DROP();                                                     \
            H2_COLD_WHY(h2g_, H2C_FLUSH);                                       \
            h2abi_v1_charge(ctx, h2acc);                                        \
            h2acc = 0;                                                          \
        }                                                                       \
    } while (0)
/* The budget left after this handler's own count, cached for the polls. */
#define H2_ACC_BUDGET_LOCALS() uint64_t h2left = h2abi_v1_budget(ctx)
#define H2_ACC_OVER() (h2acc >= h2left)
/* Settle before something that spends `extra` accesses itself (an atomic)
 * or observes the slice; a handler that polls keeps h2left in step. */
#define H2_ACC_SETTLE(extra)                                                    \
    do {                                                                        \
        (void)(extra);                                                          \
        H2_ACC_FLUSH();                                                         \
    } while (0)
#define H2_ACC_SETTLE_POLLED(extra)                                             \
    do {                                                                        \
        uint64_t n_ = h2acc + (extra);                                          \
        H2_ACC_FLUSH();                                                         \
        h2left = h2left > n_ ? h2left - n_ : 0u;                                \
    } while (0)
#else
#define H2_ACC_LOCALS() (void)0
#define H2_ACC_ONE() ((void)0)
#define H2_ACC_FLUSH() ((void)0)
#define H2_ACC_BUDGET_LOCALS() (void)0
#define H2_ACC_OVER() 0
#define H2_ACC_SETTLE(extra) ((void)0)
#define H2_ACC_SETTLE_POLLED(extra) ((void)0)
#endif

/* ---- Code-shape macros. They name the handler's locals: cpu, ctx, the
 * eight register locals, fl (EFLAGS) and fd (its defined mask). ---------- */

/* No hot-loop clocks or callbacks: counters stay in handler locals until exit. */
#if H2_PERF
#define H2_PERF_LOCALS() uint64_t h2_perf_loops_ = 0, h2_perf_x87_ = 0
#define H2_PERF_LOOP() (++h2_perf_loops_)
#define H2_PERF_X87(call) (++h2_perf_x87_, (call))
#define H2_PERF_FLUSH() h2abi_v1_profile(ctx, h2_perf_loops_, h2_perf_x87_)
#else
#define H2_PERF_LOCALS() ((void)0)
#define H2_PERF_LOOP() ((void)0)
#define H2_PERF_X87(call) (call)
#define H2_PERF_FLUSH() ((void)0)
#endif

/* Register homes. Bit i of `m` (0-7: EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI)
 * set: the handler keeps that register in a local, loaded on entry, spilled
 * at every exit and before a direct call or atomic, reloaded after them.
 * Bit 8 is EFLAGS with its defined mask. A clear bit is a register the
 * handler never writes: the emitter reads it from CpuAbi where it is used,
 * and nothing here loads, spills or reloads it, so CpuAbi holds its value
 * throughout (a direct callee or an atomic updates it there). The unused
 * local is a constant 0 that clang deletes. */
#define H2_HOME_ALL 0x1ffu
#define H2_HOMED(bit) ((h2_home_ >> (bit)) & 1u)
#define H2_LOCALS_HOMED(m)                                                      \
    enum { h2_home_ = (m) };                                                    \
    uint32_t eax = H2_HOMED(0) ? cpu->eax : 0u, ecx = H2_HOMED(1) ? cpu->ecx : 0u; \
    uint32_t edx = H2_HOMED(2) ? cpu->edx : 0u, ebx = H2_HOMED(3) ? cpu->ebx : 0u; \
    uint32_t esp = H2_HOMED(4) ? cpu->esp : 0u, ebp = H2_HOMED(5) ? cpu->ebp : 0u; \
    uint32_t esi = H2_HOMED(6) ? cpu->esi : 0u, edi = H2_HOMED(7) ? cpu->edi : 0u; \
    uint32_t fl = H2_HOMED(8) ? cpu->eflags : 0u;                               \
    uint32_t fd = H2_HOMED(8) ? cpu->eflags_defined : 0u;                       \
    uint32_t x_kind, x_route, x_target, x_resume, x_event, x_eip;               \
    H2_PERF_LOCALS(); H2_VIEW_LOCALS_(); H2_ACC_LOCALS(); (void)ctx
/* Every register in a local. */
#define H2_LOCALS() H2_LOCALS_HOMED(H2_HOME_ALL)
/* The homed locals to CpuAbi (EFLAGS with bit 1 set), and back. */
#define H2_SPILL_HOMED_() H2_SPILL_DIRTY_(H2_HOME_ALL)
/* Only the homed locals in `m` (a constant): the emitter has shown the
 * others still equal their CpuAbi fields (emit::c::dirty_at_exits). */
#define H2_SPILL_DIRTY_(m)                                                      \
    do {                                                                        \
        if (H2_HOMED(0) && ((m) & 0x1u)) cpu->eax = eax;                        \
        if (H2_HOMED(1) && ((m) & 0x2u)) cpu->ecx = ecx;                        \
        if (H2_HOMED(2) && ((m) & 0x4u)) cpu->edx = edx;                        \
        if (H2_HOMED(3) && ((m) & 0x8u)) cpu->ebx = ebx;                        \
        if (H2_HOMED(4) && ((m) & 0x10u)) cpu->esp = esp;                       \
        if (H2_HOMED(5) && ((m) & 0x20u)) cpu->ebp = ebp;                       \
        if (H2_HOMED(6) && ((m) & 0x40u)) cpu->esi = esi;                       \
        if (H2_HOMED(7) && ((m) & 0x80u)) cpu->edi = edi;                       \
        if (H2_HOMED(8) && ((m) & 0x100u)) { cpu->eflags = fl | 2u; cpu->eflags_defined = fd; } \
    } while (0)
#define H2_RELOAD_HOMED_()                                                      \
    do {                                                                        \
        if (H2_HOMED(0)) eax = cpu->eax;                                        \
        if (H2_HOMED(1)) ecx = cpu->ecx;                                        \
        if (H2_HOMED(2)) edx = cpu->edx;                                        \
        if (H2_HOMED(3)) ebx = cpu->ebx;                                        \
        if (H2_HOMED(4)) esp = cpu->esp;                                        \
        if (H2_HOMED(5)) ebp = cpu->ebp;                                        \
        if (H2_HOMED(6)) esi = cpu->esi;                                        \
        if (H2_HOMED(7)) edi = cpu->edi;                                        \
        if (H2_HOMED(8)) { fl = cpu->eflags; fd = cpu->eflags_defined; }        \
    } while (0)

/* The handler's hold on the inline RAM gate (h2fastmem.h): dropped before
 * anything that takes the context, and on every way out. Helpers that get
 * only CpuAbi or X87Abi cannot reach guest memory and may run under it. */
#define H2_GATE_DROP() (h2_gate_drop(h2_mem_, &h2g_), H2_COLD_WHY(h2g_, H2C_DROP))

/* Every way out of a handler sets the x_* locals and jumps to the one
 * shared epilogue, which commits canonical state and returns. Keeping a
 * single spill per handler (instead of one per possible fault) is most of
 * what keeps the generated objects small. */
#define H2_EPILOGUE()                                                           \
    h2_exit:                                                                    \
    H2_EPILOGUE_BODY()
#define H2_EPILOGUE_BODY()                                                      \
    H2_GATE_DROP();                                                             \
    H2_EXIT_EARLY_();                                                           \
    H2_SPILL_HOMED_();                                                          \
    /* Canonical EFLAGS has bit 1 set even where the handler left it alone. */ \
    if (!H2_HOMED(8)) cpu->eflags |= 2u;                                        \
    cpu->eip = x_eip;                                                           \
    H2_EXIT_LATE_(); H2_PERF_FLUSH();                                           \
    return h2_transfer(x_kind, x_route, x_target, x_resume, x_event)
/* What the epilogue settles with the runtime: the handler's own count
 * (accounting) and the inline RAM view's (H2_FASTMEM). H2_EXIT_CALL (the
 * default) does both in one h2abi_v1_exit, after the spill so nothing but
 * the x_* locals lives across the call; 0 is the earlier pair, a charge
 * before the spill and a release after it, kept for the equivalence tests.
 * Nothing between the two touches guest memory or the slice, so they
 * settle the same counts in the same order. */
#ifndef H2_EXIT_CALL
#define H2_EXIT_CALL 1
#endif
#ifndef H2_DIRECT_CALLS
#define H2_DIRECT_CALLS 0
#endif
/* A direct caller lends its inline RAM view to the callee (H2Direct.view),
 * which then neither acquires nor releases one: the caller has settled its
 * own count (H2_ACC_FLUSH) just before, the view is the context's one view
 * either way, and whatever the callee counts in it is settled by the
 * outermost handler's exit, or by any Rust helper the callee calls, which
 * settles first. The view's budget is the one acquire would have set again:
 * a count that carries on to it reaches the slice's end at the same access
 * as a count restarted against what is left. H2_DIRECT_VIEW=0 acquires and
 * releases in every handler, as before, for the equivalence tests. */
#ifndef H2_DIRECT_VIEW
#define H2_DIRECT_VIEW 1
#endif
#if H2_DIRECT_CALLS && H2_FASTMEM && H2_DIRECT_VIEW
#define H2_LEND_VIEW_ 1
#define H2GEN_FEATURE_DIRECT_VIEW 4u
#else
#define H2_LEND_VIEW_ 0
#define H2GEN_FEATURE_DIRECT_VIEW 0u
#endif
#if H2_ACCOUNTING
#define H2_ACC_COUNT_() h2acc
#else
#define H2_ACC_COUNT_() 0u
#endif
#if H2_EXIT_CALL && H2_LEND_VIEW_
/* A lent view is the caller's to settle: only a count of its own to charge. */
#define H2_EXIT_EARLY_() ((void)0)
#define H2_EXIT_LATE_()                                                         \
    do {                                                                        \
        if (!H2_LENT_() || H2_ACC_COUNT_()) h2abi_v1_exit(ctx, H2_ACC_COUNT_()); \
    } while (0)
#elif H2_EXIT_CALL && H2_FASTMEM
#define H2_EXIT_EARLY_() ((void)0)
#define H2_EXIT_LATE_() h2abi_v1_exit(ctx, H2_ACC_COUNT_())
#elif H2_EXIT_CALL
/* No view to settle: the charge alone, after the spill. */
#define H2_EXIT_EARLY_() ((void)0)
#define H2_EXIT_LATE_() H2_ACC_FLUSH()
#elif H2_LEND_VIEW_
#define H2_EXIT_EARLY_() H2_ACC_FLUSH()
#define H2_EXIT_LATE_()                                                         \
    do {                                                                        \
        if (!H2_LENT_()) H2_FAST_RELEASE();                                     \
    } while (0)
#else
#define H2_EXIT_EARLY_() H2_ACC_FLUSH()
#define H2_EXIT_LATE_() H2_FAST_RELEASE()
#endif
/* A hand-up (H2_DIRECT_HANDUP) releases the view unless it was lent. */
#if H2_LEND_VIEW_
#define H2_HANDUP_RELEASE_()                                                    \
    do {                                                                        \
        if (!H2_LENT_()) H2_FAST_RELEASE();                                     \
    } while (0)
#else
#define H2_HANDUP_RELEASE_() H2_FAST_RELEASE()
#endif

#define H2_EXIT_(kind, route, target, resume, event, eip)                       \
    do {                                                                        \
        H2_GATE_DROP();                                                         \
        x_kind = (kind); x_route = (route); x_target = (target);                \
        x_resume = (resume); x_event = (event); x_eip = (eip);                  \
        goto h2_exit;                                                           \
    } while (0)

/* A transfer: EIP becomes the target. */
#define H2_XFER(kind, route, target, resume)                                    \
    do {                                                                        \
        uint32_t t_ = (target);                                                 \
        H2_EXIT_((kind), (route), t_, (resume), 0u, t_);                        \
    } while (0)

/* A failed helper or guest exception: EIP stays at the instruction. */
#define H2_FAULT_EVENT(pc, st) H2_EXIT_(H2_FAULT, H2_ENTRY, 0u, 0u, (st), (pc))
#define H2_STOP_EVENT(pc, st) H2_EXIT_(H2_STOP, H2_ENTRY, 0u, 0u, (st), (pc))

/* A load or store: under the held gate the inline path (h2fastmem.h,
 * H2_HELD_TRY) gives a host address and the access is one memcpy of the
 * constant width, a single load or store. Otherwise one cold call
 * (h2_access_cold) takes the gate and accesses, or drops it and runs the
 * ordinary helper staged through b_; the handler adopts the gate state it
 * leaves, and a failure leaves through the fault exit. Either way the
 * handler's count (H2_ACC_ONE) takes the attempt. */
#define H2_LD_(T, n, dst, pc, addr, restore)                                    \
    T dst = 0;                                                                  \
    do {                                                                        \
        __label__ slow_, done_;                                                 \
        uint32_t a_ = (addr);                                                   \
        uint8_t *p_, b_[16];                                                    \
        H2FastPage pg_;                                                         \
        H2Cold c_;                                                              \
        H2_ACC_ONE();                                                           \
        H2_HELD_TRY(h2_mem_, h2g_, a_, (n), 0, pg_, p_, slow_);                 \
        memcpy(&dst, p_, (n));                                                  \
        goto done_;                                                             \
    slow_:                                                                      \
        H2_COLD_NOTE(h2g_);                                                     \
        c_ = h2_access_cold(ctx, h2_mem_, h2g_.held, (pc), a_, (n), 0, b_);     \
        h2_gate_adopt(&h2g_, c_.held);                                          \
        H2_COLD_NOTE_AFTER(h2g_);                                               \
        if (c_.status) { restore; H2_FAULT_EVENT(pc, c_.status); }              \
        memcpy(&dst, b_, (n));                                                  \
    done_:;                                                                     \
    } while (0)
#define H2_LD(T, n, dst, pc, addr) H2_LD_(T, n, dst, pc, addr, (void)0)
/* Inside a REP body: a fault reports the EFLAGS the instruction began with
 * (rfl_/rfd_, saved before the loop). */
#define H2_LDR(T, n, dst, pc, addr) H2_LD_(T, n, dst, pc, addr, (fl = rfl_, fd = rfd_))

/* A guest exception or stop recorded through h2shim_v1_event. */
#define H2_RAISE(pc, saved, code, arg)                                          \
    do {                                                                        \
        H2_GATE_DROP();                                                         \
        uint32_t e_ = h2shim_v1_event(ctx, (pc), (saved), (code), (arg));      \
        H2_FAULT_EVENT(pc, e_);                                                 \
    } while (0)
#define H2_HALT(pc, code, arg)                                                  \
    do {                                                                        \
        H2_GATE_DROP();                                                         \
        uint32_t e_ = h2shim_v1_event(ctx, (pc), (pc), (code), (arg));         \
        H2_STOP_EVENT(pc, e_);                                                  \
    } while (0)

#define H2_ST_(T, n, pc, addr, value, restore)                                  \
    do {                                                                        \
        __label__ slow_, done_;                                                 \
        T v_ = (value);                                                         \
        uint32_t a_ = (addr);                                                   \
        uint8_t *p_, b_[16];                                                    \
        H2FastPage pg_;                                                         \
        H2Cold c_;                                                              \
        H2_ACC_ONE();                                                           \
        H2_HELD_TRY(h2_mem_, h2g_, a_, (n), 1, pg_, p_, slow_);                 \
        memcpy(p_, &v_, (n));                                                   \
        goto done_;                                                             \
    slow_:                                                                      \
        memcpy(b_, &v_, (n));                                                   \
        H2_COLD_NOTE(h2g_);                                                     \
        c_ = h2_access_cold(ctx, h2_mem_, h2g_.held, (pc), a_, (n), 1, b_);     \
        h2_gate_adopt(&h2g_, c_.held);                                          \
        H2_COLD_NOTE_AFTER(h2g_);                                               \
        if (c_.status) { restore; H2_FAULT_EVENT(pc, c_.status); }              \
    done_:;                                                                     \
    } while (0)
#define H2_ST(T, n, pc, addr, value) H2_ST_(T, n, pc, addr, value, (void)0)
#define H2_STR(T, n, pc, addr, value) H2_ST_(T, n, pc, addr, value, (fl = rfl_, fd = rfd_))

/* Locals to CpuAbi and back, around a runtime call that reads and writes
 * architectural state (the atomics, a direct call). Only the homed
 * registers: CpuAbi already holds the others. */
#define H2_SYNC_OUT() H2_SPILL_HOMED_()
#define H2_SYNC_IN() H2_RELOAD_HOMED_()

/* One atomic instruction as one runtime operation: `call` is an
 * h2abi_v1_atomic_rmw/xchg/cmpxchg call writing the old memory value to
 * the local o_ (never into CpuAbi). It updates memory, flags and registers
 * under the runtime's lock; on failure nothing changed and the handler
 * leaves through the fault exit (whose spill rewrites the same values). */
#define H2_ATOMIC(pc, call)                                                     \
    uint32_t o_ = 0;                                                            \
    do {                                                                        \
        H2_GATE_DROP();                                                         \
        H2_SYNC_OUT();                                                          \
        uint32_t st_ = (call);                                                  \
        if (st_) H2_FAULT_EVENT(pc, st_);                                       \
        H2_SYNC_IN();                                                           \
    } while (0)

/* ---- x87 (ir::Intrinsic::X87): h2x87_v1_* calls on cpu->x87. --------
 * A handler with x87 code holds FIP/FDP/FOP/FCS/FDS in locals: each
 * non-control instruction sets them (H2X87_MARK, usually dead-store
 * eliminated within a block), and the epilogue writes them back, so
 * X87Abi has them at every exit. They are spilled before a helper that
 * reads them (save_image) and reloaded after one that writes them (init,
 * load_env, fnsave's commit). A failing helper leaves with x_st and x_eip
 * set through the one h2_x87_fail block: H2X87_MF raises #MF (event 14,
 * saved EIP = this waiting instruction, arg = FSW; the pointer locals
 * still describe the raiser and are spilled before the event is reported,
 * FLOAT-ABI.md §2.4), H2X87_UNMODELLED stops with code 12. */
#define H2X87_GUEST_CS 0x0008u
#define H2X87_GUEST_DS 0x0010u
#define H2X87_LOCALS()                                                          \
    uint32_t xfip_ = cpu->x87.instruction_pointer, xfdp_ = cpu->x87.data_pointer, x_st = 0; \
    uint16_t xfop_ = cpu->x87.opcode, xfcs_ = cpu->x87.instruction_selector;   \
    uint16_t xfds_ = cpu->x87.data_selector
#define H2X87_SPILL()                                                           \
    do {                                                                        \
        cpu->x87.instruction_pointer = xfip_; cpu->x87.data_pointer = xfdp_;    \
        cpu->x87.opcode = xfop_; cpu->x87.instruction_selector = xfcs_;         \
        cpu->x87.data_selector = xfds_;                                         \
    } while (0)
#define H2X87_RELOAD()                                                          \
    do {                                                                        \
        xfip_ = cpu->x87.instruction_pointer; xfdp_ = cpu->x87.data_pointer;    \
        xfop_ = cpu->x87.opcode; xfcs_ = cpu->x87.instruction_selector;         \
        xfds_ = cpu->x87.data_selector;                                         \
    } while (0)
#define H2X87_EPILOGUE()                                                        \
    h2_exit:                                                                    \
    H2X87_SPILL();                                                              \
    H2_EPILOGUE_BODY()
#define H2X87_FAIL()                                                            \
    h2_x87_fail:                                                                \
    H2_GATE_DROP();                                                             \
    H2X87_SPILL();                                                              \
    if ((x_st & H2X87_STATUS_MASK) == H2X87_MF) {                               \
        uint32_t e_ = h2shim_v1_event(ctx, x_eip, x_eip, H2SHIM_FP_ERROR, cpu->x87.status); \
        H2_FAULT_EVENT(x_eip, e_);                                              \
    } else {                                                                    \
        uint32_t e_ = h2shim_v1_event(ctx, x_eip, x_eip, H2SHIM_UNMODELLED, 0u); \
        H2_STOP_EVENT(x_eip, e_);                                               \
    }
/* FIP = pc, FOP = fop (FCS the guest's); the M form also sets FDP = ea. */
#define H2X87_MARK(pc, fop) (xfip_ = (pc), xfop_ = (uint16_t)(fop), xfcs_ = H2X87_GUEST_CS)
#define H2X87_MARKM(pc, fop, ea) (H2X87_MARK(pc, fop), xfdp_ = (ea), xfds_ = H2X87_GUEST_DS)
/* A helper call whose status is not OK leaves through h2_x87_fail. */
#define H2X87(pc, call)                                                         \
    do {                                                                        \
        uint32_t xs_ = (call);                                                  \
        if (__builtin_expect((xs_ & H2X87_STATUS_MASK) != 0, 0)) {              \
            x_st = xs_; x_eip = (pc); goto h2_x87_fail;                         \
        }                                                                       \
    } while (0)
/* X87Wait in a handler with x87 code: h2i_x87_wait through the shared
 * fail block. */
#define H2X87_WAIT(pc)                                                          \
    do {                                                                        \
        if (__builtin_expect((cpu->x87.status & H2X87_SW_ES) != 0, 0)) {        \
            x_st = H2X87_MF; x_eip = (pc); goto h2_x87_fail;                    \
        }                                                                       \
    } while (0)
/* Store phase 1: w = 1 to write (OK), 0 on SUPPRESS. */
#define H2X87_ST(pc, w, call)                                                   \
    do {                                                                        \
        uint32_t ys_ = (call);                                                  \
        if ((ys_ & H2X87_STATUS_MASK) == H2X87_SUPPRESS) (w) = 0;               \
        else { H2X87(pc, ys_); (w) = 1; }                                       \
    } while (0)
/* A compare: the payload (on OK and on SUPPRESS), and supp = 1 for
 * SUPPRESS (an unmasked exception; nothing compared or popped). */
#define H2X87_CMP(pc, payload, supp, call)                                      \
    do {                                                                        \
        uint32_t ys_ = (call);                                                  \
        (supp) = (uint8_t)((ys_ & H2X87_STATUS_MASK) == H2X87_SUPPRESS);         \
        if (!(supp)) H2X87(pc, ys_);                                            \
        (payload) = ys_ & 0x00ffffffu;                                          \
    } while (0)
/* 80-bit operands (F80, BCD) travel in h2u128 values, low 10 bytes. */
static inline uint32_t h2x87i_ld80(X87Abi *s, h2u128 v, uint32_t bcd) {
    uint8_t b[10];
    memcpy(b, &v, 10);
    return bcd ? h2x87_v1_ld_bcd(s, b) : h2x87_v1_ld_f80(s, b);
}
static inline uint32_t h2x87i_st80(X87Abi *s, uint32_t i, uint32_t bcd, h2u128 *v, uint16_t *sw) {
    uint8_t b[10] = {0};
    uint32_t r = bcd ? h2x87_v1_st_bcd(s, i, b, sw) : h2x87_v1_st_f80(s, i, b, sw);
    *v = 0;
    memcpy(v, b, 10);
    return r;
}

#define H2_FLAG(bit) ((uint8_t)((fl >> (bit)) & 1u))
#define H2_SETF(bit, v)                                                         \
    do {                                                                        \
        fl = (fl & ~(1u << (bit))) | ((uint32_t)((v) & 1u) << (bit));           \
        fd |= 1u << (bit);                                                      \
    } while (0)
#define H2_UNDEFF(bit)                                                          \
    do {                                                                        \
        fl &= ~(1u << (bit));                                                   \
        fd &= ~(1u << (bit));                                                   \
    } while (0)

/* ---- Loop safe points (RUNTIME-CONTRACT.md, "Loop safe points"). --------
 * The object behind ExecCtxOpaque * starts with a pointer to the runtime's
 * preempt word, fixed for the context's lifetime. A handler with loops reads
 * that pointer once, and each loop header polls the word with one relaxed
 * load and a branch. While it is non-zero, arriving at the header leaves
 * with JUMP/RESUME to it, state canonical as at the top of the header. The
 * header is a resume of its owner; resuming enters just past the poll, so
 * every handler call makes at least one trip round the loop. */
static inline const uint32_t *h2_preempt_word(ExecCtxOpaque *ctx) {
    const uint32_t *p;
    memcpy(&p, (const void *)ctx, sizeof p);
    return p;
}
#define H2_PREEMPT_LOCALS()                                                     \
    const uint32_t *const h2_pre_ = h2_preempt_word(ctx);                       \
    H2_ACC_BUDGET_LOCALS()
#define H2_POLLED() (__atomic_load_n(h2_pre_, __ATOMIC_RELAXED) != 0u || H2_ACC_OVER())
#if H2_COLD_STATS
#define H2_GATE_TICK_()                                                         \
    do {                                                                        \
        uint32_t h_ = h2g_.held;                                                \
        h2_gate_tick(h2_mem_, &h2g_);                                           \
        if (h_ && !h2g_.held) H2_COLD_WHY(h2g_, H2C_TICK);                      \
    } while (0)
#else
#define H2_GATE_TICK_() h2_gate_tick(h2_mem_, &h2g_)
#endif
#define H2_LOOP_POLL(header)                                                    \
    do {                                                                        \
        H2_PERF_LOOP();                                                         \
        H2_GATE_TICK_();                                                        \
        if (__builtin_expect(H2_POLLED(), 0))                                   \
            H2_XFER(H2_JUMP, H2_RESUME, (header), 0u);                          \
    } while (0)
/* After a completed REP iteration with more to come (accounting only): the
 * instruction's own PC is a resume of its owner, and leaving there with
 * ESI/EDI/ECX as they are is an interrupted string instruction. At least
 * one iteration runs per handler call, so a re-entered REP makes progress. */
#define H2_REP_POLL(pc)                                                         \
    do {                                                                        \
        if (__builtin_expect(H2_POLLED(), 0))                                   \
            H2_XFER(H2_JUMP, H2_RESUME, (pc), 0u);                              \
    } while (0)


/* ---- Direct calls (RUNTIME-CONTRACT.md §4, "Direct calls"). -------------
 * With H2_DIRECT_CALLS (on in h2 build/emit unless H2_DIRECT_CALLS=0) a
 * CALL to a function entry of this module, whose continuation is one of the
 * caller's own selectors, invokes the callee's handler as a C function
 * instead of leaving for the runner; a tail jump to one does the same and
 * returns the callee's transfer as its own. It does so only where the
 * runner's own safe point and dispatch would be no-ops: the preempt word and
 * the runtime's interrupt queue are empty, transfers are left in the
 * runner's budget, the nesting is under the context's cap, the context's
 * slot for the target says the runner resolved it to this very handler at
 * the current dispatch generation, and the target page is executable. The
 * same holds again when the callee returns: only a RETURN to exactly the
 * continuation, with ESP above the return slot, continues inline (from the
 * continuation's label, as if entered with it as start). Anything else goes
 * to the runner unchanged: before the call as the ordinary CALL exit, after
 * it as the callee's own transfer, CpuAbi as the callee left it.
 *
 * The context's second word points to the H2Direct block below, or is null
 * (the emitter's test host; a context with direct calls disabled), in
 * which case every call takes the runner. The module declares the scheme
 * as H2GEN_FEATURE_DIRECT_CALLS, so a runtime without the block refuses
 * the module. */
#ifndef H2_DIRECT_CALLS
#define H2_DIRECT_CALLS 0
#endif
#define H2GEN_FEATURE_DIRECT_CALLS 2u
#if H2_DIRECT_CALLS
#define H2GEN_FEATURE_DIRECT (H2GEN_FEATURE_DIRECT_CALLS | H2GEN_FEATURE_DIRECT_VIEW)
#else
#define H2GEN_FEATURE_DIRECT 0u
#endif
#define H2_DIRECT_SLOTS 256u
#define H2_DIRECT_MAX_DEPTH 64u
/* What the runner learnt: entry `pc` resolved to `fn` (selector pc) at
 * dispatch generation `generation`. */
typedef struct H2DirectSlot {
    uint64_t generation;
    H2Handler fn;
    uint32_t pc, pad;
} H2DirectSlot;
/* A direct CALL or JUMP still live when its handler handed a transfer up:
 * the runner replays it into its diagnostic call chain. slot = ESP. */
typedef struct H2DirectFrame {
    uint32_t kind, target, resume, slot;
} H2DirectFrame;
typedef struct H2Direct {
    const uint64_t *generation; /* the runtime's dispatch generation */
    const size_t *interrupts;   /* the runtime's queued interrupt count */
    uint64_t budget;            /* transfers the runner has left */
    uint32_t depth, cap;        /* direct-call nesting and its limit */
    uint32_t frames_len, pad;
    H2DirectFrame frames[H2_DIRECT_MAX_DEPTH];
    H2DirectSlot slots[H2_DIRECT_SLOTS];
    /* The view a direct caller lends the handler it is about to invoke
     * (module feature H2GEN_FEATURE_DIRECT_VIEW): taken, and cleared, on
     * the callee's entry; the runtime clears it before every invocation. */
    H2FastAccess *view;
} H2Direct;
#if UINTPTR_MAX == UINT64_MAX
_Static_assert(sizeof(H2DirectSlot) == 24 && sizeof(H2DirectFrame) == 16, "direct layout");
_Static_assert(sizeof(H2Direct) == 40 + 16 * 64 + 24 * 256 + 8, "direct layout");
#endif
static inline H2Direct *h2_direct(ExecCtxOpaque *ctx) {
    H2Direct *d;
    memcpy(&d, (const char *)ctx + sizeof(const uint32_t *), sizeof d);
    return d;
}
#if H2_LEND_VIEW_
/* The view lent by a direct caller, or null (called by the runner). */
static inline H2FastAccess *h2_view_borrow(ExecCtxOpaque *ctx) {
    H2Direct *d = h2_direct(ctx);
    H2FastAccess *f = d ? d->view : NULL;
    if (f) d->view = NULL;
    return f;
}
#define H2_VIEW_LOCALS_()                                                       \
    H2FastAccess *const h2_lent_ = h2_view_borrow(ctx);                         \
    H2FastAccess *h2_mem_ = h2_lent_ ? h2_lent_ : h2abi_v1_fast_acquire(ctx);   \
    H2Gate h2g_ = {0, UINT32_MAX, NULL};                                        \
    H2_COLD_WHY(h2g_, h2_lent_ ? H2C_LENT_ENTRY : H2C_ENTRY);                   \
    (void)h2_mem_
#define H2_LENT_() (h2_lent_ != 0)
#define H2_LEND_(d) ((d)->view = h2_mem_)
#else
#define H2_VIEW_LOCALS_() H2_FAST_LOCALS()
#define H2_LENT_() 0
#define H2_LEND_(d) ((void)0)
#endif
static inline uint32_t h2_direct_slot(uint32_t pc) { return (pc ^ (pc >> 8)) & (H2_DIRECT_SLOTS - 1u); }
/* The runner's check_execute fast path: the page's execute bit in the
 * inline RAM view, read under the gate. No view, a busy gate or no bit:
 * not here (the runner then decides, with its slow check too). */
static inline int h2_direct_exec(H2FastAccess *f, uint32_t pc) {
    if (!f) return 0;
    H2MemoryShared *m = f->memory;
    uint32_t unlocked = 0;
    if (!__atomic_compare_exchange_n(&m->gate, &unlocked, H2_GATE_HELD, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED))
        return 0;
    int ok = m->data.pages && (m->data.pages[pc >> 12].permissions & 4u);
    H2_GATE_RELEASE(m);
    return ok;
}
/* The runner's safe point would do nothing, and it has a transfer left. */
static inline int h2_direct_quiet(const H2Direct *d, ExecCtxOpaque *ctx) {
    return d->budget != 0u && __atomic_load_n(h2_preempt_word(ctx), __ATOMIC_RELAXED) == 0u
        && __atomic_load_n(d->interrupts, __ATOMIC_ACQUIRE) == 0u;
}
/* Before a direct CALL/JUMP: may the handler invoke `fn` for `target`? If
 * so, take a transfer and a nesting level, and record the frame at *my. */
static inline int h2_direct_enter(H2Direct *d, ExecCtxOpaque *ctx, H2FastAccess *f, H2Handler fn,
                                  uint32_t kind, uint32_t target, uint32_t resume, uint32_t esp,
                                  uint32_t *my) {
    if (!d || d->depth >= d->cap || d->depth >= H2_DIRECT_MAX_DEPTH) return 0;
    const H2DirectSlot *s = &d->slots[h2_direct_slot(target)];
    if (s->pc != target || s->fn != fn || s->generation != __atomic_load_n(d->generation, __ATOMIC_ACQUIRE))
        return 0;
    if (!h2_direct_quiet(d, ctx) || !h2_direct_exec(f, target)) return 0;
    uint32_t i = d->depth;
    H2DirectFrame fr = {kind, target, resume, esp};
    d->frames[i] = fr;
    d->depth = i + 1u;
    d->frames_len = i + 1u;
    d->budget -= 1u;
    *my = i;
    return 1;
}
/* After a direct CALL: continue at `cont` inline? Only for a RETURN there,
 * ESP above the return slot, and again a quiet safe point, a transfer left
 * and an executable continuation. */
static inline int h2_direct_back(H2Direct *d, ExecCtxOpaque *ctx, H2FastAccess *f, const CpuAbi *cpu,
                                 const TransferAbi *r, uint32_t cont, uint32_t slot, uint32_t my) {
    d->depth = my;
    if (r->kind != H2_RETURN || r->route != H2_RETURN_TARGET || r->target_pc != cont || r->event_id != 0u
        || cpu->esp <= slot)
        return 0;
    if (!h2_direct_quiet(d, ctx) || !h2_direct_exec(f, cont)) return 0;
    d->budget -= 1u;
    d->frames_len = my;
    return 1;
}
#if H2_ACCOUNTING
#define H2_ACC_REFRESH() (h2left = h2abi_v1_budget(ctx))
#else
#define H2_ACC_REFRESH() ((void)0)
#endif
/* Leave with the transfer a directly invoked handler returned: it has
 * spilled CpuAbi (and X87Abi) already, so this handler's locals are stale
 * and the epilogue must not run. Its own counters still flush. */
#define H2_DIRECT_HANDUP(r)                                                     \
    do {                                                                        \
        H2_PERF_FLUSH();                                                        \
        H2_HANDUP_RELEASE_();                                                   \
        return (r);                                                             \
    } while (0)
#if H2_DIRECT_CALLS
/* `fn` is the target's handler; `label` the continuation's; `spill` and
 * `reload` the x87 pointer locals' (or (void)0); `refresh` re-reads the
 * accounting budget in a handler that polls (or (void)0); `dirty` the
 * homed locals that may differ from CpuAbi here, which are all it spills
 * before the callee. Lazy flags are settled before it, as at any exit. */
#define H2_DCALL(fn, target, cont, label, spill, reload, refresh, dirty)       \
    do {                                                                        \
        H2_GATE_DROP();                                                         \
        H2_ACC_FLUSH();                                                         \
        H2Direct *d_ = h2_direct(ctx);                                          \
        uint32_t my_ = 0;                                                       \
        if (h2_direct_enter(d_, ctx, h2_mem_, (fn), H2_CALL, (target), (cont), esp, &my_)) { \
            H2_SPILL_DIRTY_(dirty);                                             \
            spill;                                                              \
            cpu->eip = (target);                                                \
            H2_LEND_(d_);                                                       \
            TransferAbi r_ = (fn)(cpu, ctx, (target));                          \
            if (h2_direct_back(d_, ctx, h2_mem_, cpu, &r_, (cont), esp, my_)) { \
                H2_SYNC_IN();                                                   \
                reload;                                                         \
                refresh;                                                        \
                H2_COLD_WHY(h2g_, H2C_AFTER_DCALL);                             \
                goto label;                                                     \
            }                                                                   \
            H2_DIRECT_HANDUP(r_);                                               \
        }                                                                       \
        H2_XFER(H2_CALL, H2_ENTRY, (target), (cont));                           \
    } while (0)
/* A tail jump: the target's transfer is this handler's. */
#define H2_DJUMP(fn, target, spill, dirty)                                      \
    do {                                                                        \
        H2_GATE_DROP();                                                         \
        H2_ACC_FLUSH();                                                         \
        H2Direct *d_ = h2_direct(ctx);                                          \
        uint32_t my_ = 0;                                                       \
        if (h2_direct_enter(d_, ctx, h2_mem_, (fn), H2_JUMP, (target), 0u, esp, &my_)) { \
            H2_SPILL_DIRTY_(dirty);                                             \
            spill;                                                              \
            cpu->eip = (target);                                                \
            H2_LEND_(d_);                                                       \
            TransferAbi r_ = (fn)(cpu, ctx, (target));                          \
            d_->depth = my_;                                                    \
            H2_DIRECT_HANDUP(r_);                                               \
        }                                                                       \
        H2_XFER(H2_JUMP, H2_ENTRY, (target), 0u);                               \
    } while (0)
#else
#define H2_DCALL(fn, target, cont, label, spill, reload, refresh, dirty) H2_XFER(H2_CALL, H2_ENTRY, (target), (cont))
#define H2_DJUMP(fn, target, spill, dirty) H2_XFER(H2_JUMP, H2_ENTRY, (target), 0u)
#endif

#endif
