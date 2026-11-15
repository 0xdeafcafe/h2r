#ifndef H2ABI_H
#define H2ABI_H
/* ABI v2. Generated files are disposable C11; never use native long double,
 * host pointers, host enums, or bools as guest architectural state.
 * v2 adds one requirement on the host: the object behind ExecCtxOpaque *
 * starts with a `const uint32_t *` to the context's preempt word, which
 * generated loops poll (crates/emit/RUNTIME-CONTRACT.md, "Loop safe
 * points"). A v1 host has no such prefix, so v2 code must be rejected by it.
 * Accessor symbol names keep their v1 spelling. */
#include <stdint.h>
#include <stddef.h>
#define H2_ABI_VERSION UINT32_C(2)
#define H2_CALL UINT32_C(1)
#define H2_JUMP UINT32_C(2)
#define H2_RETURN UINT32_C(3)
#define H2_FAULT UINT32_C(4)
#define H2_STOP UINT32_C(5)
#define H2_ENTRY UINT32_C(0)
#define H2_RESUME UINT32_C(1)
#define H2_RETURN_TARGET UINT32_C(2)
#define H2_HARNESS_EXIT UINT32_C(0xfffffffc)

typedef struct X87Abi {
    uint8_t st[8][10];
    uint16_t control, status, tag, opcode;
    uint32_t instruction_pointer, data_pointer;
    uint16_t instruction_selector, data_selector;
} X87Abi;
typedef struct CpuAbi {
    uint32_t abi_version, struct_size;
    uint32_t eax, ecx, edx, ebx, esp, ebp, esi, edi;
    uint32_t eip, fs_base, eflags, eflags_defined;
    X87Abi x87;
    uint8_t xmm[8][16];
    uint32_t mxcsr;
} CpuAbi;
typedef struct TransferAbi {
    uint32_t abi_version, struct_size, kind, route;
    uint32_t target_pc, resume_pc, event_id;
} TransferAbi;
typedef struct ExecCtxOpaque ExecCtxOpaque;
/* Optional H2_FASTMEM extension: view lifetime is one generated invocation.
 * Use only through h2fastmem.h; ordinary ABI v2 modules need no new symbols. */
typedef struct H2FastAccess H2FastAccess;
H2FastAccess *h2abi_v1_fast_acquire(ExecCtxOpaque *);
void h2abi_v1_fast_release(ExecCtxOpaque *);
typedef TransferAbi (*H2Handler)(CpuAbi *, ExecCtxOpaque *, uint32_t);

/* Runtime accessors have versioned symbols. The caller checks the object version before
 * registration. Handler pointers are exclusive, synchronous, and must not be
 * retained. C must never unwind through a Rust frame. */
uint32_t h2abi_v1_version(void);
/* width: 1/2/4/8/10/16; alignment: 1 normally, 16 for aligned SIMD operations.
 * Buffers contain little-endian guest bytes and must be valid for width bytes,
 * disjoint from CPU/context and any RAM view. Zero status means success; otherwise status is a
 * context event id. Reads leave output untouched on failure; writes preflight
 * the whole span. Return H2_FAULT immediately after any failing helper. */
uint32_t h2abi_v1_read(ExecCtxOpaque *, uint32_t pc, uint32_t addr,
                     uint32_t width, uint32_t alignment, uint8_t *out);
uint32_t h2abi_v1_write(ExecCtxOpaque *, uint32_t pc, uint32_t addr,
                      uint32_t width, uint32_t alignment, const uint8_t *value);
/* Accounting (module feature H2GEN_FEATURE_ACCOUNTING): the same accessors
 * without spending from the context's slice, for a module that counts its
 * own accesses, settles them with h2abi_v1_charge before leaving a handler
 * or anything that spends or observes the slice, and compares its count with
 * h2abi_v1_budget (UINT64_MAX: nothing would expire) at loop safe points. */
uint32_t h2abi_v1_read_counted(ExecCtxOpaque *, uint32_t pc, uint32_t addr,
                               uint32_t width, uint32_t alignment, uint8_t *out);
uint32_t h2abi_v1_write_counted(ExecCtxOpaque *, uint32_t pc, uint32_t addr,
                                uint32_t width, uint32_t alignment, const uint8_t *value);
void h2abi_v1_charge(ExecCtxOpaque *, uint64_t accesses);
uint64_t h2abi_v1_budget(ExecCtxOpaque *);

/* Optional H2_PERF=1 generated counters, batched once at each handler exit.
 * Extension symbol only: ordinary ABI v2 modules do not require it. */
void h2abi_v1_profile(ExecCtxOpaque *, uint64_t loop_polls, uint64_t x87_helpers);

/* Define once in each generated module's registration object. Registration
 * passes h2gen_abi_version to register_generated_object. */
#define H2_DEFINE_GENERATED_ABI_NAMED(name) \
    uint32_t name(void) { return H2_ABI_VERSION; }
/* Default probes are local to each translation unit. Use _NAMED to export a
 * unique symbol for Rust registration; never rely on a shared weak definition. */
#define H2_DEFINE_GENERATED_ABI() \
    static inline uint32_t h2gen_abi_version(void) { return H2_ABI_VERSION; }
#define H2_ATOMIC_ADD 1u
#define H2_ATOMIC_ADC 2u
#define H2_ATOMIC_SUB 3u
#define H2_ATOMIC_SBB 4u
#define H2_ATOMIC_AND 5u
#define H2_ATOMIC_OR 6u
#define H2_ATOMIC_XOR 7u
#define H2_ATOMIC_INC 8u
#define H2_ATOMIC_DEC 9u
#define H2_ATOMIC_NEG 10u
#define H2_ATOMIC_NOT 11u
#define H2_ATOMIC_BTS 12u
#define H2_ATOMIC_BTR 13u
#define H2_ATOMIC_BTC 14u
/* Atomics validate the whole read/write span and hold one backing lock through
 * memory, flags and results. Width is 1/2/4; output is the old operand, zero
 * extended to u32. CPU and context must be disjoint. Output must be disjoint
 * from context but may point into CpuAbi. Its store happens after the CPU
 * operation and overrides any earlier update of that field. On a guest
 * fault memory, CPU and output are unchanged. XADD uses ADD and the old result.
 * BTS/BTR/BTC require width 2/4 and an already adjusted memory address; operand
 * selects a bit modulo the operand width. xchg/NOT preserve flags;
 * cmpxchg8b uses EDX:EAX and ECX:EBX and changes only ZF. */
uint32_t h2abi_v1_atomic_rmw(CpuAbi *, ExecCtxOpaque *, uint32_t pc, uint32_t addr,
                           uint32_t width, uint32_t op, uint32_t operand, uint32_t *out);
uint32_t h2abi_v1_xchg(CpuAbi *, ExecCtxOpaque *, uint32_t pc, uint32_t addr,
                     uint32_t width, uint32_t value, uint32_t *out);
uint32_t h2abi_v1_cmpxchg(CpuAbi *, ExecCtxOpaque *, uint32_t pc, uint32_t addr,
                        uint32_t width, uint32_t replacement, uint32_t *out);
uint32_t h2abi_v1_cmpxchg8b(CpuAbi *, ExecCtxOpaque *, uint32_t pc, uint32_t addr);

/* Generated-module shims (crates/emit RUNTIME-CONTRACT.md). h2shim_v1_event
 * records a guest exception (codes 1-7, 13; latched like a failed access) or a
 * stop (8-12) and returns its non-zero event id; saved_eip is the EIP the
 * exception reports (the next instruction for traps). h2shim_v1_unsupported
 * copies both strings and returns a stop's event id. h2shim_v1_rdtsc returns
 * the guest's time-stamp counter (rdtsc at pc): guest time at the Xbox CPU's
 * 733 MHz, strictly increasing across every thread's reads. These macros and
 * prototypes are token-identical to the emitter's h2gen.h, so both headers can
 * be included together. */
#define H2SHIM_DIVIDE_ERROR 1u
#define H2SHIM_BREAKPOINT 2u
#define H2SHIM_INVALID_OPCODE 3u
#define H2SHIM_INTERRUPT 4u
#define H2SHIM_OVERFLOW 5u
#define H2SHIM_BOUND_RANGE 6u
#define H2SHIM_GENERAL_PROTECTION 7u
#define H2SHIM_HALT 8u
#define H2SHIM_BAD_ENTRY 9u
#define H2SHIM_UNKNOWN_TARGET 10u
#define H2SHIM_DESCENT_ERROR 11u
#define H2SHIM_UNMODELLED 12u
#define H2SHIM_SIMD_EXCEPTION 13u
#define H2SHIM_FP_ERROR 14u
uint32_t h2shim_v1_event(ExecCtxOpaque *, uint32_t pc, uint32_t saved_eip, uint32_t code, uint32_t arg);
uint32_t h2shim_v1_unsupported(ExecCtxOpaque *, uint32_t pc, const char *mnemonic, const char *detail);
uint64_t h2shim_v1_rdtsc(ExecCtxOpaque *, uint32_t pc);

static inline TransferAbi h2_transfer(uint32_t kind, uint32_t route,
                                     uint32_t target, uint32_t resume,
                                     uint32_t event) {
    TransferAbi t = {H2_ABI_VERSION, sizeof(TransferAbi), kind, route,
                     target, resume, event};
    return t;
}
_Static_assert(sizeof(X87Abi) == 100 && _Alignof(X87Abi) == 4, "X87Abi v1");
_Static_assert(sizeof(CpuAbi) == 288 && _Alignof(CpuAbi) == 4, "CpuAbi v1");
_Static_assert(sizeof(TransferAbi) == 28 && _Alignof(TransferAbi) == 4, "TransferAbi v1");
_Static_assert(offsetof(CpuAbi, eax) == 8, "CpuAbi GPRs");
_Static_assert(offsetof(CpuAbi, eip) == 40, "CpuAbi EIP");
_Static_assert(offsetof(CpuAbi, x87) == 56, "CpuAbi x87");
_Static_assert(offsetof(CpuAbi, xmm) == 156, "CpuAbi XMM");
_Static_assert(offsetof(CpuAbi, mxcsr) == 284, "CpuAbi MXCSR");
#endif
