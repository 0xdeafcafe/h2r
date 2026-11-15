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

typedef unsigned __int128 h2u128;
/* SSE floating-point reference semantics (h2s_*), written next to this
 * header by crates/emit. */
#include "h2simd.h"

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
typedef struct H2GenModule {
    uint32_t abi_version, entry_count, resume_count, reserved;
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

static inline uint32_t h2i_x87_init(CpuAbi *cpu) {
    cpu->x87.control = 0x037f; cpu->x87.status = 0; cpu->x87.tag = 0xffff; cpu->x87.opcode = 0;
    cpu->x87.instruction_pointer = 0; cpu->x87.data_pointer = 0;
    cpu->x87.instruction_selector = 0; cpu->x87.data_selector = 0;
    return 0;
}
static inline uint32_t h2i_x87_read_control(CpuAbi *cpu, uint16_t *out) { *out = cpu->x87.control; return 0; }
static inline uint32_t h2i_x87_write_control(CpuAbi *cpu, uint16_t v) { cpu->x87.control = v; return 0; }
static inline uint32_t h2i_x87_read_status(CpuAbi *cpu, uint16_t *out) { *out = cpu->x87.status; return 0; }
static inline uint32_t h2i_x87_clear_exceptions(CpuAbi *cpu) { cpu->x87.status &= 0x7f00u; return 0; }
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
#define H2_SSE_SS(op)                                                           \
    static inline uint32_t h2i_sse_##op(CpuAbi *cpu, h2u128 a, uint32_t b, h2u128 *out) { \
        return H2_SSE_RES(h2s_##op(&cpu->mxcsr, a, b, out));                   \
    }
#define H2_SSE_PS(op)                                                           \
    static inline uint32_t h2i_sse_##op(CpuAbi *cpu, h2u128 a, h2u128 b, h2u128 *out) { \
        return H2_SSE_RES(h2s_##op(&cpu->mxcsr, a, b, out));                   \
    }
H2_SSE_SS(addss) H2_SSE_PS(addps) H2_SSE_SS(subss) H2_SSE_PS(subps)
H2_SSE_SS(mulss) H2_SSE_PS(mulps) H2_SSE_SS(divss) H2_SSE_PS(divps)
H2_SSE_SS(minss) H2_SSE_PS(minps) H2_SSE_SS(maxss) H2_SSE_PS(maxps)
H2_SSE_SS(sqrtss) H2_SSE_SS(cvtsi2ss)
static inline uint32_t h2i_sse_sqrtps(CpuAbi *cpu, h2u128 b, h2u128 *out) {
    return H2_SSE_RES(h2s_sqrtps(&cpu->mxcsr, b, out));
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

/* ---- Code-shape macros. They name the handler's locals: cpu, ctx, the
 * eight register locals, fl (EFLAGS) and fd (its defined mask). ---------- */

#define H2_LOCALS()                                                             \
    uint32_t eax = cpu->eax, ecx = cpu->ecx, edx = cpu->edx, ebx = cpu->ebx;    \
    uint32_t esp = cpu->esp, ebp = cpu->ebp, esi = cpu->esi, edi = cpu->edi;    \
    uint32_t fl = cpu->eflags, fd = cpu->eflags_defined;                       \
    uint32_t x_kind, x_route, x_target, x_resume, x_event, x_eip;               \
    (void)ctx

/* Every way out of a handler sets the x_* locals and jumps to the one
 * shared epilogue, which commits canonical state and returns. Keeping a
 * single spill per handler (instead of one per possible fault) is most of
 * what keeps the generated objects small. */
#define H2_EPILOGUE()                                                           \
    h2_exit:                                                                    \
    cpu->eax = eax; cpu->ecx = ecx; cpu->edx = edx; cpu->ebx = ebx;             \
    cpu->esp = esp; cpu->ebp = ebp; cpu->esi = esi; cpu->edi = edi;             \
    cpu->eflags = fl | 2u; cpu->eflags_defined = fd; cpu->eip = x_eip;          \
    return h2_transfer(x_kind, x_route, x_target, x_resume, x_event)

#define H2_EXIT_(kind, route, target, resume, event, eip)                       \
    do {                                                                        \
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
        uint32_t st_ = h2abi_v1_read(ctx, (pc), (addr), (n), 1u, b_);           \
        if (st_) { restore; H2_FAULT_EVENT(pc, st_); }                          \
        memcpy(&dst, b_, (n));                                                  \
    } while (0)
#define H2_LD(T, n, dst, pc, addr) H2_LD_(T, n, dst, pc, addr, (void)0)
/* Inside a REP body: a fault reports the EFLAGS the instruction began with
 * (rfl_/rfd_, saved before the loop). */
#define H2_LDR(T, n, dst, pc, addr) H2_LD_(T, n, dst, pc, addr, (fl = rfl_, fd = rfd_))

/* A guest exception or stop recorded through h2shim_v1_event. */
#define H2_RAISE(pc, saved, code, arg)                                          \
    do {                                                                        \
        uint32_t e_ = h2shim_v1_event(ctx, (pc), (saved), (code), (arg));      \
        H2_FAULT_EVENT(pc, e_);                                                 \
    } while (0)
#define H2_HALT(pc, code, arg)                                                  \
    do {                                                                        \
        uint32_t e_ = h2shim_v1_event(ctx, (pc), (pc), (code), (arg));         \
        H2_STOP_EVENT(pc, e_);                                                  \
    } while (0)

#define H2_ST_(T, n, pc, addr, value, restore)                                  \
    do {                                                                        \
        T v_ = (value);                                                         \
        uint8_t b_[16];                                                         \
        memcpy(b_, &v_, (n));                                                   \
        uint32_t st_ = h2abi_v1_write(ctx, (pc), (addr), (n), 1u, b_);          \
        if (st_) { restore; H2_FAULT_EVENT(pc, st_); }                          \
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
        H2_SYNC_OUT();                                                          \
        uint32_t st_ = (call);                                                  \
        if (st_) H2_FAULT_EVENT(pc, st_);                                       \
        H2_SYNC_IN();                                                           \
    } while (0)

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

#endif
