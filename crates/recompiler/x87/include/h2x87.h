/* h2x87.h: x87 runtime helpers, ABI v1 (docs/lifter/FLOAT-ABI.md §1-§4).
 *
 * Implemented by crates/recompiler/x87 (src/abi.rs, `ffi`). Every helper takes the
 * guest thread's X87Abi (CpuAbi.x87, h2abi.h v1): eight physical 10-byte
 * registers, TOP in status bits 11..13, the 2-bit tag word.
 *
 * Return value: the top byte is a status, the low 24 bits a payload (only
 * h2x87_v1_cmp has one).
 *   H2X87_OK          completed. An unmasked IE/DE/ZE or stack fault left
 *                     only the status word changed (its IE/DE/ZE/SF flags,
 *                     ES, B, C1 for a stack fault); #MF comes at the next
 *                     waiting instruction.
 *   H2X87_MF          ES was set on entry: nothing ran. Raise
 *                     H2SHIM_FP_ERROR with saved_eip = this instruction,
 *                     arg = status.
 *   H2X87_UNMODELLED  unmasked OE/UE/PE (UE: any tiny result, exact or
 *                     not): nothing ran. Stop with H2SHIM_UNMODELLED.
 *   H2X87_SUPPRESS    unmasked pre-computation fault, from two helpers:
 *                     stores: nothing changed. Do not write the
 *                       destination; call h2x87_v1_store_commit with
 *                       pop = 0.
 *                     h2x87_v1_cmp: the status word is already recorded,
 *                       nothing else changed; see its comment.
 *
 * A null pointer or out-of-range selector aborts the process.
 */
#ifndef H2X87_H
#define H2X87_H

#include <stdint.h>
#include "h2abi.h"

#define H2X87_ABI_VERSION 1u

#define H2X87_OK 0x00000000u
#define H2X87_MF 0x01000000u
#define H2X87_UNMODELLED 0x02000000u
#define H2X87_SUPPRESS 0x03000000u
#define H2X87_STATUS_MASK 0xff000000u

/* Event code for H2X87_MF (#MF, vector 16). Token-identical to the
 * definitions in h2abi.h and h2gen.h, so the headers can be included
 * together, and present here for an h2abi.h that predates it. */
#define H2SHIM_FP_ERROR 14u

/* Status-word bits. */
#define H2X87_SW_IE 0x0001u
#define H2X87_SW_DE 0x0002u
#define H2X87_SW_ZE 0x0004u
#define H2X87_SW_OE 0x0008u
#define H2X87_SW_UE 0x0010u
#define H2X87_SW_PE 0x0020u
#define H2X87_SW_SF 0x0040u
#define H2X87_SW_ES 0x0080u
#define H2X87_SW_C0 0x0100u
#define H2X87_SW_C1 0x0200u
#define H2X87_SW_C2 0x0400u
#define H2X87_SW_TOP 0x3800u
#define H2X87_SW_C3 0x4000u
#define H2X87_SW_B 0x8000u

/* h2x87_v1_cmp payload with to_flags != 0. */
#define H2X87_EFL_CF 0x0001u
#define H2X87_EFL_PF 0x0004u
#define H2X87_EFL_ZF 0x0040u

/* h2x87_v1_arith op; OR H2X87_ARITH_POP for the ...p forms. */
#define H2X87_ADD 0u
#define H2X87_SUB 1u
#define H2X87_SUBR 2u
#define H2X87_MUL 3u
#define H2X87_DIV 4u
#define H2X87_DIVR 5u
#define H2X87_ARITH_POP 0x100u

/* Source kinds for arith and cmp; src carries the raw bits or ST index. */
#define H2X87_SRC_ST 0u
#define H2X87_SRC_F32 1u
#define H2X87_SRC_F64 2u
#define H2X87_SRC_I16 3u
#define H2X87_SRC_I32 4u

/* h2x87_v1_cmp kind. FTST is COM against SRC_F32 with src = 0. */
#define H2X87_CMP_COM 0u
#define H2X87_CMP_UCOM 1u
#define H2X87_CMP_ICOM 2u

/* h2x87_v1_unary op (X87Unary order). */
#define H2X87_SQRT 0u
#define H2X87_ABS 1u
#define H2X87_CHS 2u
#define H2X87_RNDINT 3u
#define H2X87_SIN 4u
#define H2X87_COS 5u
#define H2X87_TAN 6u
#define H2X87_PATAN 7u
#define H2X87_YL2X 8u
#define H2X87_YL2XP1 9u
#define H2X87_X2M1 10u
#define H2X87_SCALE 11u
#define H2X87_XTRACT 12u
#define H2X87_SINCOS 13u
#define H2X87_PREM 14u
#define H2X87_PREM1 15u

/* h2x87_v1_ld_const which (X87LoadConst order). */
#define H2X87_C_ONE 0u
#define H2X87_C_Z 1u
#define H2X87_C_PI 2u
#define H2X87_C_L2E 3u
#define H2X87_C_L2T 4u
#define H2X87_C_LG2 5u
#define H2X87_C_LN2 6u

/* Environment ops. Image is 28 (ENV) or 108 (SAVE) bytes, 32-bit
 * protected-mode layout. */
#define H2X87_FNSTENV 0u
#define H2X87_FLDENV 1u
#define H2X87_FNSAVE 2u
#define H2X87_FRSTOR 3u
#define H2X87_ENV_LEN 28u
#define H2X87_SAVE_LEN 108u

/* Loads (push). */
uint32_t h2x87_v1_ld_f32(X87Abi *s, uint32_t bits);
uint32_t h2x87_v1_ld_f64(X87Abi *s, uint64_t bits);
uint32_t h2x87_v1_ld_f80(X87Abi *s, const uint8_t m[10]);
uint32_t h2x87_v1_ld_i16(X87Abi *s, int16_t v);
uint32_t h2x87_v1_ld_i32(X87Abi *s, int32_t v);
uint32_t h2x87_v1_ld_i64(X87Abi *s, int64_t v);
uint32_t h2x87_v1_ld_bcd(X87Abi *s, const uint8_t m[10]);
uint32_t h2x87_v1_ld_const(X87Abi *s, uint32_t which);
uint32_t h2x87_v1_ld_st(X87Abi *s, uint32_t i);

/* Stores, phase 1 (convert; X87Abi unchanged). On OK write *value to the
 * guest, then on success call store_commit(sw, pop). On SUPPRESS write
 * nothing and call store_commit(sw, 0). On MF/UNMODELLED call nothing.
 * A guest fault between the phases leaves the x87 state untouched. */
uint32_t h2x87_v1_st_f32(X87Abi *s, uint32_t i, uint32_t *value, uint16_t *sw);
uint32_t h2x87_v1_st_f64(X87Abi *s, uint32_t i, uint64_t *value, uint16_t *sw);
uint32_t h2x87_v1_st_f80(X87Abi *s, uint32_t i, uint8_t value[10], uint16_t *sw);
uint32_t h2x87_v1_st_i16(X87Abi *s, uint32_t i, int16_t *value, uint16_t *sw);
uint32_t h2x87_v1_st_i32(X87Abi *s, uint32_t i, int32_t *value, uint16_t *sw);
uint32_t h2x87_v1_st_i64(X87Abi *s, uint32_t i, int64_t *value, uint16_t *sw);
uint32_t h2x87_v1_st_bcd(X87Abi *s, uint32_t i, uint8_t value[10], uint16_t *sw);
/* Truncating int32 store regardless of RC: for CRT overrides only (FISTTP
 * is not a P6 instruction). */
uint32_t h2x87_v1_st_i32_trunc(X87Abi *s, uint32_t i, int32_t *value, uint16_t *sw);
/* Stores, phase 3. */
uint32_t h2x87_v1_store_commit(X87Abi *s, uint16_t sw, uint32_t pop);
/* FST/FSTP ST(dst): register-only, one step. */
uint32_t h2x87_v1_st_st(X87Abi *s, uint32_t dst, uint32_t pop);

/* _ftol2 override (0x322b0c): truncate ST(0) to int64 and pop. NaN,
 * infinity or |x| >= 2^63 give 0x8000000000000000 with IE; inexact sets
 * PE. EDX:EAX = *out. */
uint32_t h2x87_v1_ftol2(X87Abi *s, uint64_t *out);

/* Arithmetic. Memory forms use dst_i = 0. */
uint32_t h2x87_v1_arith(X87Abi *s, uint32_t op, uint32_t dst_i, uint32_t src_kind, uint64_t src);
uint32_t h2x87_v1_unary(X87Abi *s, uint32_t op);
/* Compare ST(0) with a source; pop 0..2.
 * H2X87_OK, to_flags = 0: payload is C3|C2|C0 as status-word bits (also
 *   written to status, C1 cleared).
 * H2X87_OK, to_flags = 1 (FCOMI forms): payload is ZF|PF|CF; the caller
 *   replaces ZF/PF/CF with it, clears OF/SF/AF, marks all six defined.
 * H2X87_SUPPRESS (unmasked IE/DE or stack fault): the compare did not
 *   happen and nothing popped. to_flags = 0: payload is the unchanged
 *   C3|C2|C0, so a fused fnstsw/test/jcc may still use the low bits.
 *   to_flags = 1: payload 0; the caller keeps ZF/PF/CF, clears OF/SF/AF.
 * H2X87_MF: nothing changed; raise H2SHIM_FP_ERROR. */
uint32_t h2x87_v1_cmp(X87Abi *s, uint32_t kind, uint32_t src_kind, uint64_t src, uint32_t pop, uint32_t to_flags);
uint32_t h2x87_v1_xam(X87Abi *s);

/* Stack. */
uint32_t h2x87_v1_xch(X87Abi *s, uint32_t i);
uint32_t h2x87_v1_free(X87Abi *s, uint32_t i, uint32_t pop);
uint32_t h2x87_v1_adj_top(X87Abi *s, int32_t delta);
uint32_t h2x87_v1_cmov(X87Abi *s, uint32_t i, uint32_t take);

/* Control. FLDCW stores (cw & 0x1f3f) | 0x40 and recomputes ES/B. */
uint32_t h2x87_v1_ldcw(X87Abi *s, uint16_t cw);
/* FWAIT, and the pending-#MF check MMX entry and EMMS make. */
uint32_t h2x87_v1_wait(X87Abi *s);
uint32_t h2x87_v1_clex(X87Abi *s);
uint32_t h2x87_v1_init(X87Abi *s);

/* FNSTENV/FNSAVE: phase 1 builds the image (X87Abi unchanged), the caller
 * writes ENV_LEN/SAVE_LEN bytes, then phase 3 masks all (FNSTENV) or
 * FNINITs (FNSAVE). Materialise FIP/FDP/FOP/FCS/FDS before phase 1. */
uint32_t h2x87_v1_save_image(const X87Abi *s, uint32_t op, uint8_t img[108]);
uint32_t h2x87_v1_save_commit(X87Abi *s, uint32_t op);
/* FLDENV/FRSTOR from an image already read from the guest (call
 * h2x87_v1_wait first). Normalises CW, recomputes ES/B and the tags. */
uint32_t h2x87_v1_load_env(X87Abi *s, uint32_t op, const uint8_t img[108]);

_Static_assert(sizeof(X87Abi) == 100 && _Alignof(X87Abi) == 4, "h2x87 v1: X87Abi");
_Static_assert(__builtin_offsetof(X87Abi, st) == 0, "h2x87 v1: st");
_Static_assert(__builtin_offsetof(X87Abi, control) == 80, "h2x87 v1: control");
_Static_assert(__builtin_offsetof(X87Abi, status) == 82, "h2x87 v1: status");
_Static_assert(__builtin_offsetof(X87Abi, tag) == 84, "h2x87 v1: tag");
_Static_assert(__builtin_offsetof(X87Abi, opcode) == 86, "h2x87 v1: opcode");
_Static_assert(__builtin_offsetof(X87Abi, instruction_pointer) == 88, "h2x87 v1: fip");
_Static_assert(__builtin_offsetof(X87Abi, data_pointer) == 92, "h2x87 v1: fdp");
_Static_assert(__builtin_offsetof(X87Abi, instruction_selector) == 96, "h2x87 v1: fcs");
_Static_assert(__builtin_offsetof(X87Abi, data_selector) == 98, "h2x87 v1: fds");
_Static_assert(H2SHIM_FP_ERROR == 14u, "h2x87 v1: #MF event code");

#endif
