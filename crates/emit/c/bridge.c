/* Test bridge: provides the ABI symbols generated code links against by
 * forwarding to function pointers a Rust host installs at load time. This
 * lets a generated module be loaded as a dylib into a test process without
 * the real runtime. The production build links the runtime instead. */
#include "h2gen.h"

typedef struct H2Bridge {
    uint32_t (*read)(ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint8_t *);
    uint32_t (*write)(ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, const uint8_t *);
    uint32_t (*event)(ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t);
    uint32_t (*unsupported)(ExecCtxOpaque *, uint32_t, const char *, const char *);
    uint64_t (*rdtsc)(ExecCtxOpaque *, uint32_t);
    uint32_t (*rmw)(CpuAbi *, ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *);
    uint32_t (*xchg)(CpuAbi *, ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *);
    uint32_t (*cmpxchg)(CpuAbi *, ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *);
} H2Bridge;

static H2Bridge bridge;

void h2bridge_install(const H2Bridge *b) { bridge = *b; }

/* The table's layout, so a Rust copy can check it before installing:
 * slots 0-7 give each callback's offset in declaration order, 8 the size. */
size_t h2bridge_layout(uint32_t slot) {
    static const size_t layout[] = {
        offsetof(H2Bridge, read),  offsetof(H2Bridge, write), offsetof(H2Bridge, event),
        offsetof(H2Bridge, unsupported), offsetof(H2Bridge, rdtsc), offsetof(H2Bridge, rmw),
        offsetof(H2Bridge, xchg),  offsetof(H2Bridge, cmpxchg), sizeof(H2Bridge),
    };
    return slot < sizeof layout / sizeof layout[0] ? layout[slot] : 0;
}

uint32_t h2abi_v1_version(void) { return H2_ABI_VERSION; }

uint32_t h2abi_v1_read(ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width, uint32_t alignment,
                       uint8_t *out) {
    return bridge.read(c, pc, addr, width, alignment, out);
}

uint32_t h2abi_v1_write(ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width, uint32_t alignment,
                        const uint8_t *value) {
    return bridge.write(c, pc, addr, width, alignment, value);
}

uint32_t h2shim_v1_event(ExecCtxOpaque *c, uint32_t pc, uint32_t saved_eip, uint32_t code, uint32_t arg) {
    return bridge.event(c, pc, saved_eip, code, arg);
}

uint32_t h2shim_v1_unsupported(ExecCtxOpaque *c, uint32_t pc, const char *m, const char *d) {
    return bridge.unsupported(c, pc, m, d);
}

uint64_t h2shim_v1_rdtsc(ExecCtxOpaque *c, uint32_t pc) { return bridge.rdtsc(c, pc); }

uint32_t h2abi_v1_atomic_rmw(CpuAbi *cpu, ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width,
                             uint32_t op, uint32_t operand, uint32_t *out) {
    return bridge.rmw(cpu, c, pc, addr, width, op, operand, out);
}

uint32_t h2abi_v1_xchg(CpuAbi *cpu, ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width, uint32_t value,
                       uint32_t *out) {
    return bridge.xchg(cpu, c, pc, addr, width, value, out);
}

uint32_t h2abi_v1_cmpxchg(CpuAbi *cpu, ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width,
                          uint32_t replacement, uint32_t *out) {
    return bridge.cmpxchg(cpu, c, pc, addr, width, replacement, out);
}
