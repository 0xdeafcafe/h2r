/* Public ABI conformance probe; contains no game-derived code. */
#include "h2abi.h"

TransferAbi h2_test_c_handler(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t pc) {
    uint8_t bytes[4] = {0x78, 0x56, 0x34, 0x12};
    uint32_t event = h2abi_v1_write(ctx, pc, cpu->esi, 4, 1, bytes);
    if (event) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, event);
    event = h2abi_v1_read(ctx, pc, cpu->esi, 4, 1, bytes);
    if (event) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, event);
    cpu->eax = ((uint32_t)bytes[0]) | ((uint32_t)bytes[1] << 8)
             | ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
    cpu->x87.st[7][9] = 0xab;
    cpu->xmm[7][15] = 0xcd;
    cpu->mxcsr = 0x1f80;
    return h2_transfer(H2_CALL, H2_ENTRY, 0x12345678, pc, 0);
}

TransferAbi h2_test_c_calls_rust(H2Handler f, CpuAbi *cpu,
                               ExecCtxOpaque *ctx, uint32_t pc) {
    TransferAbi t = f(cpu, ctx, pc);
    /* Observe fields on both sides of the struct-return convention. */
    if (t.target_pc != 0x12345678 || t.kind != H2_CALL || cpu->eax != 0x87654321)
        return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, 0);
    t.resume_pc = 0xabcdef01;
    return t;
}

TransferAbi h2_test_c_memory_widths(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t pc) {
    const uint32_t widths[] = {1, 2, 4, 8, 10, 16};
    uint8_t value[16], result[16];
    for (uint32_t i = 0; i < 16; ++i) value[i] = (uint8_t)(0xa0 + i);
    for (uint32_t i = 0; i < 6; ++i) {
        uint32_t addr = cpu->esi + i * 32;
        uint32_t event = h2abi_v1_write(ctx, pc, addr, widths[i], 1, value);
        if (event) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, event);
        event = h2abi_v1_read(ctx, pc, addr, widths[i], 1, result);
        if (event) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, event);
        for (uint32_t j = 0; j < widths[i]; ++j) {
            if (result[j] != value[j]) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, 0);
        }
    }
    return h2_transfer(H2_STOP, H2_ENTRY, 0, 0, 0);
}

H2_DEFINE_GENERATED_ABI_NAMED(h2gen_abi_version)
uint32_t h2gen_bad_abi_version(void) { return H2_ABI_VERSION + 1; }

TransferAbi h2_test_c_atomics(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t pc) {
    uint32_t old = 0, status;
    status = h2abi_v1_atomic_rmw(cpu, ctx, pc, cpu->esi, 4, H2_ATOMIC_ADD, 1, &old);
    if (!status) status = h2abi_v1_xchg(cpu, ctx, pc, cpu->esi, 4, 42, &old);
    cpu->eax = 42;
    if (!status) status = h2abi_v1_cmpxchg(cpu, ctx, pc, cpu->esi, 4, 43, &old);
    cpu->eax = 43; cpu->edx = 0; cpu->ebx = 44; cpu->ecx = 0;
    if (!status) status = h2abi_v1_cmpxchg8b(cpu, ctx, pc, cpu->esi);
    return h2_transfer(status ? H2_FAULT : H2_STOP, H2_ENTRY, 0, 0, status);
}

TransferAbi h2_test_c_atomic_aliases(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t pc) {
    uint32_t status;
    if (h2abi_v1_version() != H2_ABI_VERSION)
        return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, 0);
    cpu->eax = 9;
    status = h2abi_v1_xchg(cpu, ctx, pc, cpu->esi, 4, cpu->eax, &cpu->eax);
    if (status) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, status);
    if (cpu->eax != 44) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, 0);
    status = h2abi_v1_atomic_rmw(cpu, ctx, pc, cpu->esi, 4, H2_ATOMIC_ADD, 1, &cpu->ebx);
    if (status) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, status);
    if (cpu->ebx != 9) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, 0);
    cpu->eax = 10;
    status = h2abi_v1_cmpxchg(cpu, ctx, pc, cpu->esi, 4, 11, &cpu->eax);
    if (status) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, status);
    if (cpu->eax != 10 || !(cpu->eflags & 0x40))
        return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, 0);
    /* Failed compare loads EAX and subsequently writes the same old value. */
    status = h2abi_v1_cmpxchg(cpu, ctx, pc, cpu->esi, 4, 12, &cpu->eax);
    if (status) return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, status);
    if (cpu->eax != 11 || (cpu->eflags & 0x40))
        return h2_transfer(H2_FAULT, H2_ENTRY, 0, 0, 0);
    /* An access fault must leave the aliased CPU output untouched. */
    cpu->eax = 0x12345678;
    status = h2abi_v1_xchg(cpu, ctx, pc, 0, 4, 9, &cpu->eax);
    return h2_transfer(status ? H2_FAULT : H2_STOP, H2_ENTRY, 0, 0, status);
}
