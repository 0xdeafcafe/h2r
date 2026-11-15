/* A hand-written module in the shape crates/emit generates, for the runtime's
 * RUNTIME-CONTRACT.md tests. Contains no game-derived code. The H2Gen tables
 * and the shim prototypes mirror the emitter's h2gen.h. */
#include "h2abi.h"

uint32_t h2shim_v1_event(ExecCtxOpaque *, uint32_t pc, uint32_t saved_eip, uint32_t code, uint32_t arg);
uint32_t h2shim_v1_unsupported(ExecCtxOpaque *, uint32_t pc, const char *mnemonic, const char *detail);
/* Repeated from h2gen.h on purpose: must stay compatible with h2abi.h. */
#define H2SHIM_DIVIDE_ERROR 1u
#define H2SHIM_SIMD_EXCEPTION 13u

typedef struct H2GenEntry {
    uint32_t addr, kind;
    H2Handler fn;
    uint32_t (*abi_version)(void);
} H2GenEntry;
typedef struct H2GenResume {
    uint32_t pc, owner, start;
} H2GenResume;
typedef struct H2GenModule {
    uint32_t abi_version, entry_count, resume_count, reserved;
    const H2GenEntry *entries;
    const H2GenResume *resumes;
} H2GenModule;

/* Guest layout (synthetic):
 *   MAIN  0x401000  call LEAF; resume 0x401005 runs the mode in EBX
 *         0x401010  interior label (kind 1): ECX = 0x10; ret
 *         0x401020  interior label (kind 1): rep movsb
 *   LEAF  0x402000  EDX = 0: EAX = 41; ret. EDX = 1: #DE at 0x402003.
 *                   EDX = 2: unsupported at 0x402003.
 *   A     0x403000  call C; its continuation is B's entry
 *   B     0x403005  EAX = 1; ret
 *   C     0x404000  ret */
#define MAIN 0x401000u
#define MAIN_RESUME 0x401005u
#define MAIN_LABEL 0x401010u
#define MAIN_REP 0x401020u
#define LEAF 0x402000u
#define FA 0x403000u
#define FB 0x403005u
#define FC 0x404000u

#define FAULT_AT(pc_, e) do { cpu->eip = (pc_); return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, (e)); } while (0)
#define STOP_AT(pc_, e) do { cpu->eip = (pc_); return h2_transfer(H2_STOP, H2_ENTRY, 0, 0, (e)); } while (0)

static uint32_t push32(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t pc, uint32_t v) {
    uint8_t b[4] = {(uint8_t)v, (uint8_t)(v >> 8), (uint8_t)(v >> 16), (uint8_t)(v >> 24)};
    uint32_t e = h2abi_v1_write(ctx, pc, cpu->esp - 4, 4, 1, b);
    if (!e) cpu->esp -= 4;
    return e;
}

static TransferAbi call(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t pc, uint32_t target, uint32_t next) {
    uint32_t e = push32(cpu, ctx, pc, next);
    if (e) FAULT_AT(pc, e);
    cpu->eip = target;
    return h2_transfer(H2_CALL, H2_ENTRY, target, next, 0);
}

static TransferAbi ret(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t pc, uint32_t cleanup) {
    uint8_t b[4];
    uint32_t e = h2abi_v1_read(ctx, pc, cpu->esp, 4, 1, b);
    if (e) FAULT_AT(pc, e);
    uint32_t target = (uint32_t)b[0] | (uint32_t)b[1] << 8 | (uint32_t)b[2] << 16 | (uint32_t)b[3] << 24;
    cpu->esp += 4 + cleanup;
    cpu->eip = target;
    return h2_transfer(H2_RETURN, H2_RETURN_TARGET, target, 0, 0);
}

/* rep movsb at MAIN_REP, one byte per iteration. A faulting iteration leaves
 * the completed iterations' ESI/EDI/ECX, EFLAGS as before, eip = the insn. */
static TransferAbi rep_movsb(CpuAbi *cpu, ExecCtxOpaque *ctx) {
    const uint32_t pc = MAIN_REP;
    while (cpu->ecx) {
        uint8_t byte;
        uint32_t e = h2abi_v1_read(ctx, pc, cpu->esi, 1, 1, &byte);
        if (!e) e = h2abi_v1_write(ctx, pc, cpu->edi, 1, 1, &byte);
        if (e) FAULT_AT(pc, e);
        cpu->esi += 1;
        cpu->edi += 1;
        cpu->ecx -= 1;
    }
    return ret(cpu, ctx, pc + 2, 0);
}

TransferAbi h2fn_00401000(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {
    switch (start) {
    case MAIN:
        return call(cpu, ctx, MAIN, LEAF, MAIN_RESUME);
    case MAIN_LABEL:
        cpu->ecx = 0x10;
        return ret(cpu, ctx, MAIN_LABEL, 0);
    case MAIN_REP:
        return rep_movsb(cpu, ctx);
    case MAIN_RESUME:
        break;
    default:
        STOP_AT(start, h2shim_v1_event(ctx, start, start, 9, start));
    }
    const uint32_t pc = MAIN_RESUME + 1;
    cpu->eax += 1;
    switch (cpu->ebx) {
    case 0:
        return ret(cpu, ctx, pc, 0);
    case 1: /* div with a zero divisor */
        FAULT_AT(pc, h2shim_v1_event(ctx, pc, pc, 1, 0));
    case 2: /* hlt */
        STOP_AT(pc, h2shim_v1_event(ctx, pc, pc, 8, 0));
    case 3: /* int 0x2d: a trap, reports the next instruction */
        FAULT_AT(pc, h2shim_v1_event(ctx, pc, pc + 2, 4, 0x2d));
    case 4: { /* a broken handler that ignores the exception's id */
        (void)h2shim_v1_event(ctx, pc, pc, 3, 0);
        cpu->eip = MAIN_LABEL;
        return h2_transfer(H2_JUMP, H2_ENTRY, MAIN_LABEL, 0, 0);
    }
    case 5: /* a code from a newer generator */
        FAULT_AT(pc, h2shim_v1_event(ctx, pc, pc, 99, 7));
    case 8: /* an unmasked SSE exception (#XM) */
        FAULT_AT(pc, h2shim_v1_event(ctx, pc, pc, H2SHIM_SIMD_EXCEPTION, 0));
    case 6: /* every stop code */
        STOP_AT(pc, h2shim_v1_event(ctx, pc, pc, cpu->ecx, cpu->edx));
    default:
        STOP_AT(pc, h2shim_v1_unsupported(ctx, pc, "fxsave", "x87: fxsave is not lowered"));
    }
}

TransferAbi h2fn_00402000(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {
    if (start != LEAF) STOP_AT(start, h2shim_v1_event(ctx, start, start, 9, start));
    const uint32_t pc = LEAF + 3;
    switch (cpu->edx) {
    case 1:
        FAULT_AT(pc, h2shim_v1_event(ctx, pc, pc, 1, 0));
    case 2:
        STOP_AT(pc, h2shim_v1_unsupported(ctx, pc, "aaa", "integer: BCD adjust"));
    default:
        cpu->eax = 41;
        return ret(cpu, ctx, pc, 0);
    }
}

TransferAbi h2fn_00403000(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {
    if (start != FA) STOP_AT(start, h2shim_v1_event(ctx, start, start, 9, start));
    return call(cpu, ctx, FA, FC, FB);
}

TransferAbi h2fn_00403005(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {
    if (start != FB) STOP_AT(start, h2shim_v1_event(ctx, start, start, 9, start));
    cpu->eax = 1;
    return ret(cpu, ctx, FB + 5, 0);
}

TransferAbi h2fn_00404000(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {
    if (start != FC) STOP_AT(start, h2shim_v1_event(ctx, start, start, 9, start));
    return ret(cpu, ctx, FC, 0);
}

H2_DEFINE_GENERATED_ABI_NAMED(h2gen_abi_version_fixture_a)
H2_DEFINE_GENERATED_ABI_NAMED(h2gen_abi_version_fixture_b)
H2_DEFINE_GENERATED_ABI_NAMED(h2gen_fixture_module_abi_version)

static const H2GenEntry entries[] = {
    {MAIN, 0, h2fn_00401000, h2gen_abi_version_fixture_a},
    {MAIN_LABEL, 1, h2fn_00401000, h2gen_abi_version_fixture_a},
    {MAIN_REP, 1, h2fn_00401000, h2gen_abi_version_fixture_a},
    {LEAF, 0, h2fn_00402000, h2gen_abi_version_fixture_a},
    {FA, 0, h2fn_00403000, h2gen_abi_version_fixture_b},
    {FB, 0, h2fn_00403005, h2gen_abi_version_fixture_b},
    {FC, 0, h2fn_00404000, h2gen_abi_version_fixture_b},
    {0, 0, 0, 0},
};
static const H2GenResume resumes[] = {
    {MAIN_RESUME, MAIN, MAIN_RESUME},
    {FB, FB, FB},
    {0, 0, 0},
};
const H2GenModule h2gen_fixture_module = {
    H2_ABI_VERSION, sizeof entries / sizeof entries[0] - 1, sizeof resumes / sizeof resumes[0] - 1, 0,
    entries, resumes,
};
