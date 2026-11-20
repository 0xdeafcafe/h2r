/* Record protocol shared by the test harnesses (soft_harness.c runs h2simd.h,
 * sse_harness.c runs the host's real SSE unit) and tests/common/mod.rs.
 *
 * Input record, 40 bytes little-endian:
 *   u8 op, u8 pred, u16 zero, u32 mxcsr, u128 a, u128 b
 * Output record, 24 bytes:
 *   u8 status (0 ok, 1 #XM), u8[3] zero, u32 mxcsr after, u128 result
 * result is 0 when status is 1.
 *
 * Operand use per op: a is the destination register's old value, b the
 * source. Scalar ops read b's low lane; comiss/ucomiss compare a's low lane
 * with b's; cvtsi2ss converts b's low 32 bits as an int32; cvtpi2ps b's low
 * 64 bits as two int32s; cvt[t]ss2si, cvt[t]ps2pi, rcpps and rsqrtps read
 * only b. comiss results are EFLAGS & (ZF|PF|CF). */
#ifndef H2SIMD_PROTOCOL_H
#define H2SIMD_PROTOCOL_H

enum {
    OP_ADDSS, OP_ADDPS, OP_SUBSS, OP_SUBPS, OP_MULSS, OP_MULPS, OP_DIVSS,
    OP_DIVPS, OP_SQRTSS, OP_SQRTPS, OP_MINSS, OP_MINPS, OP_MAXSS, OP_MAXPS,
    OP_CMPSS, OP_CMPPS, OP_COMISS, OP_UCOMISS, OP_CVTSI2SS, OP_CVTSS2SI,
    OP_CVTTSS2SI, OP_CVTPI2PS, OP_CVTPS2PI, OP_CVTTPS2PI, OP_RCPSS, OP_RCPPS,
    OP_RSQRTSS, OP_RSQRTPS, OP_COUNT
};

#define IN_SIZE 40
#define OUT_SIZE 24

#endif
