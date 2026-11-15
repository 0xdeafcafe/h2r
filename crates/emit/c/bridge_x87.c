/* Test bridge, x87 part: the h2x87_v1_* symbols, forwarding to the
 * functions a Rust host installs with h2x87bridge_install (index = the
 * order below, which is h2x87.h's declaration order; see
 * emit::host::X87_BRIDGE_SYMBOLS). Before anything is installed, or for a
 * null entry, a helper answers H2X87_UNMODELLED, so generated code stops
 * with code 12 instead of calling through null. Appended to bridge.c.
 * Generated from crates/x87/include/h2x87.h; regenerate when it changes. */

static void *h2x87_bridge[35];

void h2x87bridge_install(void *const *fns, uint32_t n) {
    for (uint32_t i = 0; i < n && i < 35; i++) h2x87_bridge[i] = fns[i];
}

#define H2X87_FWD(i, name, P, A)                                                \
    uint32_t name P {                                                           \
        typedef uint32_t(*f_) P;                                                \
        f_ f = (f_)h2x87_bridge[i];                                             \
        return f ? f A : H2X87_UNMODELLED;                                      \
    }

H2X87_FWD(0, h2x87_v1_ld_f32, (X87Abi *s, uint32_t bits), (s, bits))
H2X87_FWD(1, h2x87_v1_ld_f64, (X87Abi *s, uint64_t bits), (s, bits))
H2X87_FWD(2, h2x87_v1_ld_f80, (X87Abi *s, const uint8_t m[10]), (s, m))
H2X87_FWD(3, h2x87_v1_ld_i16, (X87Abi *s, int16_t v), (s, v))
H2X87_FWD(4, h2x87_v1_ld_i32, (X87Abi *s, int32_t v), (s, v))
H2X87_FWD(5, h2x87_v1_ld_i64, (X87Abi *s, int64_t v), (s, v))
H2X87_FWD(6, h2x87_v1_ld_bcd, (X87Abi *s, const uint8_t m[10]), (s, m))
H2X87_FWD(7, h2x87_v1_ld_const, (X87Abi *s, uint32_t which), (s, which))
H2X87_FWD(8, h2x87_v1_ld_st, (X87Abi *s, uint32_t i), (s, i))
H2X87_FWD(9, h2x87_v1_st_f32, (X87Abi *s, uint32_t i, uint32_t *value, uint16_t *sw), (s, i, value, sw))
H2X87_FWD(10, h2x87_v1_st_f64, (X87Abi *s, uint32_t i, uint64_t *value, uint16_t *sw), (s, i, value, sw))
H2X87_FWD(11, h2x87_v1_st_f80, (X87Abi *s, uint32_t i, uint8_t value[10], uint16_t *sw), (s, i, value, sw))
H2X87_FWD(12, h2x87_v1_st_i16, (X87Abi *s, uint32_t i, int16_t *value, uint16_t *sw), (s, i, value, sw))
H2X87_FWD(13, h2x87_v1_st_i32, (X87Abi *s, uint32_t i, int32_t *value, uint16_t *sw), (s, i, value, sw))
H2X87_FWD(14, h2x87_v1_st_i64, (X87Abi *s, uint32_t i, int64_t *value, uint16_t *sw), (s, i, value, sw))
H2X87_FWD(15, h2x87_v1_st_bcd, (X87Abi *s, uint32_t i, uint8_t value[10], uint16_t *sw), (s, i, value, sw))
H2X87_FWD(16, h2x87_v1_st_i32_trunc, (X87Abi *s, uint32_t i, int32_t *value, uint16_t *sw), (s, i, value, sw))
H2X87_FWD(17, h2x87_v1_store_commit, (X87Abi *s, uint16_t sw, uint32_t pop), (s, sw, pop))
H2X87_FWD(18, h2x87_v1_st_st, (X87Abi *s, uint32_t dst, uint32_t pop), (s, dst, pop))
H2X87_FWD(19, h2x87_v1_ftol2, (X87Abi *s, uint64_t *out), (s, out))
H2X87_FWD(20, h2x87_v1_arith, (X87Abi *s, uint32_t op, uint32_t dst_i, uint32_t src_kind, uint64_t src), (s, op, dst_i, src_kind, src))
H2X87_FWD(21, h2x87_v1_unary, (X87Abi *s, uint32_t op), (s, op))
H2X87_FWD(22, h2x87_v1_cmp, (X87Abi *s, uint32_t kind, uint32_t src_kind, uint64_t src, uint32_t pop, uint32_t to_flags), (s, kind, src_kind, src, pop, to_flags))
H2X87_FWD(23, h2x87_v1_xam, (X87Abi *s), (s))
H2X87_FWD(24, h2x87_v1_xch, (X87Abi *s, uint32_t i), (s, i))
H2X87_FWD(25, h2x87_v1_free, (X87Abi *s, uint32_t i, uint32_t pop), (s, i, pop))
H2X87_FWD(26, h2x87_v1_adj_top, (X87Abi *s, int32_t delta), (s, delta))
H2X87_FWD(27, h2x87_v1_cmov, (X87Abi *s, uint32_t i, uint32_t take), (s, i, take))
H2X87_FWD(28, h2x87_v1_ldcw, (X87Abi *s, uint16_t cw), (s, cw))
H2X87_FWD(29, h2x87_v1_wait, (X87Abi *s), (s))
H2X87_FWD(30, h2x87_v1_clex, (X87Abi *s), (s))
H2X87_FWD(31, h2x87_v1_init, (X87Abi *s), (s))
H2X87_FWD(32, h2x87_v1_save_image, (const X87Abi *s, uint32_t op, uint8_t img[108]), (s, op, img))
H2X87_FWD(33, h2x87_v1_save_commit, (X87Abi *s, uint32_t op), (s, op))
H2X87_FWD(34, h2x87_v1_load_env, (X87Abi *s, uint32_t op, const uint8_t img[108]), (s, op, img))
