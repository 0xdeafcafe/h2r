/* Out-of-line helpers for the Cranelift fast tier (crates/recompiler/clif,
 * docs/lifter/cranelift-tier.md section 3.9). Not derived from any disc:
 * the same for every image, so it compiles once and stays cached.
 *
 * Each wrapper calls the very h2i_sse_* function clang's tier inlines
 * (h2gen.h, with the H2_SSE_NATIVE guard), so both tiers compute the same
 * bits. Cranelift passes operands in a buffer, one per 16-byte slot at
 * their own width, little-endian; the result goes to `out`. The return
 * value is h2i_sse_*'s: 0, or the H2SHIM_* fault code. */
#include "h2gen.h"

#define H2C_ARG(T, n) T a##n; memcpy(&a##n, args + 16 * (n), sizeof a##n)
#define H2C_OUT(T) T r = 0
#define H2C_DONE() memcpy(out, &r, sizeof r); return s

#define H2C_SS(op)                                                              \
    uint32_t h2clif_sse_##op(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {  \
        H2C_ARG(h2u128, 0); H2C_ARG(uint32_t, 1); H2C_OUT(h2u128);              \
        uint32_t s = h2i_sse_##op(cpu, a0, a1, &r); H2C_DONE();                 \
    }
#define H2C_PS(op)                                                              \
    uint32_t h2clif_sse_##op(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {  \
        H2C_ARG(h2u128, 0); H2C_ARG(h2u128, 1); H2C_OUT(h2u128);                \
        uint32_t s = h2i_sse_##op(cpu, a0, a1, &r); H2C_DONE();                 \
    }
#define H2C_P1(op)                                                              \
    uint32_t h2clif_sse_##op(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {  \
        H2C_ARG(h2u128, 0); H2C_OUT(h2u128);                                    \
        uint32_t s = h2i_sse_##op(cpu, a0, &r); H2C_DONE();                     \
    }

H2C_SS(addss) H2C_SS(subss) H2C_SS(mulss) H2C_SS(divss) H2C_SS(sqrtss)
H2C_SS(minss) H2C_SS(maxss) H2C_SS(rcpss) H2C_SS(rsqrtss) H2C_SS(cvtsi2ss)
H2C_PS(addps) H2C_PS(subps) H2C_PS(mulps) H2C_PS(divps) H2C_PS(minps) H2C_PS(maxps)
H2C_P1(sqrtps) H2C_P1(rcpps) H2C_P1(rsqrtps)

uint32_t h2clif_sse_cmpss(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(h2u128, 0); H2C_ARG(uint32_t, 1); H2C_ARG(uint8_t, 2); H2C_OUT(h2u128);
    uint32_t s = h2i_sse_cmpss(cpu, a0, a1, a2, &r); H2C_DONE();
}
uint32_t h2clif_sse_cmpps(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(h2u128, 0); H2C_ARG(h2u128, 1); H2C_ARG(uint8_t, 2); H2C_OUT(h2u128);
    uint32_t s = h2i_sse_cmpps(cpu, a0, a1, a2, &r); H2C_DONE();
}
uint32_t h2clif_sse_comiss(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(uint32_t, 0); H2C_ARG(uint32_t, 1); H2C_OUT(uint32_t);
    uint32_t s = h2i_sse_comiss(cpu, a0, a1, &r); H2C_DONE();
}
uint32_t h2clif_sse_ucomiss(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(uint32_t, 0); H2C_ARG(uint32_t, 1); H2C_OUT(uint32_t);
    uint32_t s = h2i_sse_ucomiss(cpu, a0, a1, &r); H2C_DONE();
}
uint32_t h2clif_sse_cvtss2si(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(uint32_t, 0); H2C_OUT(uint32_t);
    uint32_t s = h2i_sse_cvtss2si(cpu, a0, &r); H2C_DONE();
}
uint32_t h2clif_sse_cvttss2si(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(uint32_t, 0); H2C_OUT(uint32_t);
    uint32_t s = h2i_sse_cvttss2si(cpu, a0, &r); H2C_DONE();
}
uint32_t h2clif_sse_cvtpi2ps(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(h2u128, 0); H2C_ARG(uint64_t, 1); H2C_OUT(h2u128);
    uint32_t s = h2i_sse_cvtpi2ps(cpu, a0, a1, &r); H2C_DONE();
}
uint32_t h2clif_sse_cvtps2pi(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(uint64_t, 0); H2C_OUT(uint64_t);
    uint32_t s = h2i_sse_cvtps2pi(cpu, a0, &r); H2C_DONE();
}
uint32_t h2clif_sse_cvttps2pi(CpuAbi *cpu, const uint8_t *args, uint8_t *out) {
    H2C_ARG(uint64_t, 0); H2C_OUT(uint64_t);
    uint32_t s = h2i_sse_cvttps2pi(cpu, a0, &r); H2C_DONE();
}
