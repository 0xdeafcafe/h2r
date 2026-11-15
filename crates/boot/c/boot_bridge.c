/* Boot bridge: the ABI symbols a generated module imports, forwarded to
 * function pointers the Rust host installs at load time (the runtime's
 * h2abi_v1_* exports and crates/boot's h2shim_v1_* implementations). This
 * keeps the generated dylib free of link-time dependencies on the host
 * executable's symbol table. Not derived from any disc. */
#include "h2gen.h"

typedef struct H2BootBridge {
    uint32_t (*read)(ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint8_t *);
    uint32_t (*write)(ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, const uint8_t *);
    uint32_t (*event)(ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t);
    uint32_t (*unsupported)(ExecCtxOpaque *, uint32_t, const char *, const char *);
    uint64_t (*rdtsc)(ExecCtxOpaque *, uint32_t);
    uint32_t (*rmw)(CpuAbi *, ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *);
    uint32_t (*xchg)(CpuAbi *, ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *);
    uint32_t (*cmpxchg)(CpuAbi *, ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *);
    uint32_t (*cmpxchg8b)(CpuAbi *, ExecCtxOpaque *, uint32_t, uint32_t);
} H2BootBridge;

static H2BootBridge bridge;

void h2boot_install(const H2BootBridge *b) { bridge = *b; }

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
uint32_t h2abi_v1_cmpxchg8b(CpuAbi *cpu, ExecCtxOpaque *c, uint32_t pc, uint32_t addr) {
    return bridge.cmpxchg8b(cpu, c, pc, addr);
}
