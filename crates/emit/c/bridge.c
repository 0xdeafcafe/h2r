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
    void (*profile)(ExecCtxOpaque *, uint64_t, uint64_t);
    H2FastAccess *(*fast_acquire)(ExecCtxOpaque *);
    void (*fast_release)(ExecCtxOpaque *);
    uint32_t (*read_counted)(ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, uint8_t *);
    uint32_t (*write_counted)(ExecCtxOpaque *, uint32_t, uint32_t, uint32_t, uint32_t, const uint8_t *);
    void (*charge)(ExecCtxOpaque *, uint64_t);
    uint64_t (*budget)(ExecCtxOpaque *);
} H2Bridge;

static H2Bridge bridge;

/* Version the install symbol and check size before reading the table: a
 * prefix layout check cannot distinguish an old size from a new offset. */
uint32_t h2bridge_install_v2(const H2Bridge *b, size_t size) {
    if (size != sizeof bridge) return 0;
    bridge = *b;
    return 1;
}

/* The table's layout, so a Rust copy can check it before installing:
 * slots 0-14 give each callback's offset in declaration order, 15 the size. */
size_t h2bridge_layout_v2(uint32_t slot) {
    static const size_t layout[] = {
        offsetof(H2Bridge, read),  offsetof(H2Bridge, write), offsetof(H2Bridge, event),
        offsetof(H2Bridge, unsupported), offsetof(H2Bridge, rdtsc), offsetof(H2Bridge, rmw),
        offsetof(H2Bridge, xchg),  offsetof(H2Bridge, cmpxchg), offsetof(H2Bridge, profile), offsetof(H2Bridge, fast_acquire), offsetof(H2Bridge, fast_release),
        offsetof(H2Bridge, read_counted), offsetof(H2Bridge, write_counted), offsetof(H2Bridge, charge),
        offsetof(H2Bridge, budget), sizeof(H2Bridge),
    };
    return slot < sizeof layout / sizeof layout[0] ? layout[slot] : 0;
}

/* Original eight-callback clients remain usable, but cannot opt into the
 * new extensions. Never copy an expanded table from an unversioned caller. */
static void legacy_profile(ExecCtxOpaque *c, uint64_t l, uint64_t x) { (void)c; (void)l; (void)x; }
static H2FastAccess *legacy_fast_acquire(ExecCtxOpaque *c) { (void)c; return NULL; }
static void legacy_fast_release(ExecCtxOpaque *c) { (void)c; }
/* An older installer has no uncharged accessors: its charged ones count each
 * access once, and the handler's own count is dropped. */
static uint32_t legacy_read_counted(ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width, uint32_t alignment,
                                    uint8_t *out) {
    return bridge.read(c, pc, addr, width, alignment, out);
}
static uint32_t legacy_write_counted(ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width,
                                     uint32_t alignment, const uint8_t *value) {
    return bridge.write(c, pc, addr, width, alignment, value);
}
static void legacy_charge(ExecCtxOpaque *c, uint64_t n) { (void)c; (void)n; }
static uint64_t legacy_budget(ExecCtxOpaque *c) { (void)c; return UINT64_MAX; }
void h2bridge_install(const void *b) {
    memcpy(&bridge, b, offsetof(H2Bridge, profile));
    bridge.profile = legacy_profile;
    bridge.fast_acquire = legacy_fast_acquire;
    bridge.fast_release = legacy_fast_release;
    bridge.read_counted = legacy_read_counted;
    bridge.write_counted = legacy_write_counted;
    bridge.charge = legacy_charge;
    bridge.budget = legacy_budget;
}
size_t h2bridge_layout(uint32_t slot) {
    return slot < 8 ? h2bridge_layout_v2(slot) : slot == 8 ? offsetof(H2Bridge, profile) : 0;
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

uint32_t h2abi_v1_read_counted(ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width, uint32_t alignment,
                               uint8_t *out) {
    return bridge.read_counted(c, pc, addr, width, alignment, out);
}

uint32_t h2abi_v1_write_counted(ExecCtxOpaque *c, uint32_t pc, uint32_t addr, uint32_t width, uint32_t alignment,
                                const uint8_t *value) {
    return bridge.write_counted(c, pc, addr, width, alignment, value);
}

void h2abi_v1_charge(ExecCtxOpaque *c, uint64_t accesses) { bridge.charge(c, accesses); }

uint64_t h2abi_v1_budget(ExecCtxOpaque *c) { return bridge.budget(c); }

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

void h2abi_v1_profile(ExecCtxOpaque *c, uint64_t loops, uint64_t x87) { bridge.profile(c, loops, x87); }

H2FastAccess *h2abi_v1_fast_acquire(ExecCtxOpaque *c) { return bridge.fast_acquire(c); }
void h2abi_v1_fast_release(ExecCtxOpaque *c) { bridge.fast_release(c); }
