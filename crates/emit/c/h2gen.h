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
 * h2abi_v1_read/write_counted. Handlers settle the count
 * with h2abi_v1_charge: on every way out (the shared epilogue)
 * and before anything that spends or observes the slice (H2_ACC_SETTLE).
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
        if (h2acc) { H2_GATE_DROP(); h2abi_v1_charge(ctx, h2acc); h2acc = 0; }  \
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

#define H2_LOCALS()                                                             \
    uint32_t eax = cpu->eax, ecx = cpu->ecx, edx = cpu->edx, ebx = cpu->ebx;    \
    uint32_t esp = cpu->esp, ebp = cpu->ebp, esi = cpu->esi, edi = cpu->edi;    \
    uint32_t fl = cpu->eflags, fd = cpu->eflags_defined;                       \
    uint32_t x_kind, x_route, x_target, x_resume, x_event, x_eip;               \
    H2_PERF_LOCALS(); H2_FAST_LOCALS(); H2_ACC_LOCALS(); (void)ctx

/* The handler's hold on the inline RAM gate (h2fastmem.h): dropped before
 * anything that takes the context, and on every way out. Helpers that get
 * only CpuAbi or X87Abi cannot reach guest memory and may run under it. */
#define H2_GATE_DROP() h2_gate_drop(h2_mem_, &h2g_)

/* Every way out of a handler sets the x_* locals and jumps to the one
 * shared epilogue, which commits canonical state and returns. Keeping a
 * single spill per handler (instead of one per possible fault) is most of
 * what keeps the generated objects small. */
#define H2_EPILOGUE()                                                           \
    h2_exit:                                                                    \
    H2_EPILOGUE_BODY()
#define H2_EPILOGUE_BODY()                                                      \
    H2_GATE_DROP();                                                             \
    H2_ACC_FLUSH();                                                             \
    cpu->eax = eax; cpu->ecx = ecx; cpu->edx = edx; cpu->ebx = ebx;             \
    cpu->esp = esp; cpu->ebp = ebp; cpu->esi = esi; cpu->edi = edi;             \
    cpu->eflags = fl | 2u; cpu->eflags_defined = fd; cpu->eip = x_eip;          \
    H2_FAST_RELEASE(); H2_PERF_FLUSH();                                                         \
    return h2_transfer(x_kind, x_route, x_target, x_resume, x_event)

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

#define H2_LD_(T, n, dst, pc, addr, restore)                                    \
    T dst = 0;                                                                  \
    do {                                                                        \
        uint8_t b_[16];                                                         \
        uint32_t a_ = (addr);                                                   \
        H2_ACC_ONE();                                                           \
        if (!h2_held_read(h2_mem_, &h2g_, a_, (n), b_)) {                       \
            H2_GATE_DROP();                                                     \
            uint32_t st_ = H2_MEM_READ_SLOW(ctx, (pc), a_, (n), 1u, b_);        \
            if (st_) { restore; H2_FAULT_EVENT(pc, st_); }                      \
        }                                                                       \
        memcpy(&dst, b_, (n));                                                  \
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
        T v_ = (value);                                                         \
        uint8_t b_[16];                                                         \
        uint32_t a_ = (addr);                                                   \
        memcpy(b_, &v_, (n));                                                   \
        H2_ACC_ONE();                                                           \
        if (!h2_held_write(h2_mem_, &h2g_, a_, (n), b_)) {                      \
            H2_GATE_DROP();                                                     \
            uint32_t st_ = H2_MEM_WRITE_SLOW(ctx, (pc), a_, (n), 1u, b_);       \
            if (st_) { restore; H2_FAULT_EVENT(pc, st_); }                      \
        }                                                                       \
    } while (0)
#define H2_ST(T, n, pc, addr, value) H2_ST_(T, n, pc, addr, value, (void)0)
#define H2_STR(T, n, pc, addr, value) H2_ST_(T, n, pc, addr, value, (fl = rfl_, fd = rfd_))

/* Locals to CpuAbi and back, around a runtime call that reads and writes
 * architectural state (the atomics). */
#define H2_SYNC_OUT()                                                           \
    do {                                                                        \
        cpu->eax = eax; cpu->ecx = ecx; cpu->edx = edx; cpu->ebx = ebx;         \
        cpu->esp = esp; cpu->ebp = ebp; cpu->esi = esi; cpu->edi = edi;         \
        cpu->eflags = fl | 2u; cpu->eflags_defined = fd;                        \
    } while (0)
#define H2_SYNC_IN()                                                            \
    do {                                                                        \
        eax = cpu->eax; ecx = cpu->ecx; edx = cpu->edx; ebx = cpu->ebx;         \
        esp = cpu->esp; ebp = cpu->ebp; esi = cpu->esi; edi = cpu->edi;         \
        fl = cpu->eflags; fd = cpu->eflags_defined;                             \
    } while (0)

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
#define H2_LOOP_POLL(header)                                                    \
    do {                                                                        \
        H2_PERF_LOOP();                                                         \
        h2_gate_tick(h2_mem_, &h2g_);                                           \
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

#endif
