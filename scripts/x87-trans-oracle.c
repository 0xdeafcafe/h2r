/* Independent synthetic oracle, MPFR 4.2+. No game inputs.
 * Compile with: cc scripts/x87-trans-oracle.c $(pkg-config --cflags --libs mpfr gmp)
 * stdout is the public fixture file consumed by x87/tests/trans_oracle.rs.
 * Directed 1024-bit intervals must agree on BOTH the 64-bit nearest value
 * and its rounding direction, otherwise generation fails.
 */
#include <assert.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <mpfr.h>

typedef struct { uint64_t sig; uint16_t se; } f80;
typedef struct { f80 value; unsigned flags; } result;

static uint64_t state = UINT64_C(0xd6e8feb86659fd93);
static uint64_t next(void) {
    state ^= state << 13; state ^= state >> 7; state ^= state << 17;
    return state;
}
static f80 input(int lo, int hi) {
    int e = lo + (int)(next() % (unsigned)(hi - lo + 1));
    f80 v = {next() | (UINT64_C(1) << 63),
             (uint16_t)((e + 16383) | ((next() & 1) << 15))};
    return v;
}
static void set(mpfr_t out, f80 in) {
    mpfr_set_uj(out, in.sig, MPFR_RNDN);
    mpfr_mul_2si(out, out, (in.se & 0x7fff) - 16383 - 63, MPFR_RNDN);
    if (in.se & 0x8000) mpfr_neg(out, out, MPFR_RNDN);
}
static result rounded(const mpfr_t in, mpfr_prec_t precision) {
    mpfr_t r, sig;
    mpfr_init2(r, precision); mpfr_init2(sig, 1024);
    mpfr_set(r, in, MPFR_RNDN);
    assert(mpfr_regular_p(r));
    long exp = mpfr_get_exp(r);
    mpfr_abs(sig, r, MPFR_RNDN);
    mpfr_mul_2si(sig, sig, 64 - exp, MPFR_RNDN);
    result out = {{mpfr_get_uj(sig, MPFR_RNDN),
                  (uint16_t)(exp - 1 + 16383 + (mpfr_signbit(r) ? 0x8000 : 0))},
                  (mpfr_equal_p(r, in) ? 0 : 0x20) |
                  (mpfr_cmpabs(r, in) > 0 ? 0x200 : 0)};
    mpfr_clear(r); mpfr_clear(sig);
    return out;
}
int main(void) {
    mpfr_t y, x, lo, hi, reference;
    mpfr_inits2(1024, y, x, lo, hi, reference, (mpfr_ptr)0);
    puts("# MPFR 4.2+ directed 1024-bit intervals; seed d6e8feb86659fd93");
    puts("# op y.sig y.se x.sig x.se result64.sig se flags result53.sig se flags result24.sig se flags");
    for (unsigned op = 0; op < 2; ++op) {
        for (unsigned i = 0; i < 512; ++i) {
            f80 a = input(-1074, 1023), b = input(op == 1 ? -1138 : -16382, op == 1 ? -1 : 16383);
            if (op == 0) {
                b.se &= 0x7fff;
                if (i < 277) b.se = (uint16_t)(16383 - 149 + i);
            } else {
                int exponent = -1138 + (int)(i * 1137 / 511);
                b.se = (uint16_t)(exponent + 16383) | (b.se & 0x8000);
            }
            /* Near-one inputs exercise cancellation in log2. */
            if (op == 0 && i < 4) {
                b.sig = i & 1 ? UINT64_MAX : (UINT64_C(1) << 63) + 1;
                b.se = i & 1 ? 16382 : 16383;
            }
            /* Exact log identities expose independent PC halfway cases. */
            if (op == 0 && i >= 4 && i <= 10) {
                b.sig = UINT64_C(1) << 63; b.se = 16384; /* x=2 */
                a.se = 16383;
                a.sig = (UINT64_C(1) << 63) + (i <= 7 ? (UINT64_C(1) << 10) : (UINT64_C(1) << 39));
                if (i == 4 || i == 8) --a.sig;
                if (i == 6 || i == 10) ++a.sig;
                if (i == 7) a.sig = UINT64_C(1) << 63;
            }
            if (op == 1 && (i == 6 || i == 7)) {
                b.sig = UINT64_C(1) << 63; b.se = 16383 | (i == 7 ? 0x8000 : 0);
            }
            set(y, a); set(x, b);
            if (op == 0) {
                mpfr_log2(lo, x, MPFR_RNDD);
                mpfr_log2(hi, x, MPFR_RNDU);
                if (mpfr_signbit(y)) mpfr_swap(lo, hi);
                mpfr_mul(lo, lo, y, MPFR_RNDD);
                mpfr_mul(hi, hi, y, MPFR_RNDU);
            } else {
                mpfr_exp2m1(lo, x, MPFR_RNDD);
                mpfr_exp2m1(hi, x, MPFR_RNDU);
            }
            result l = rounded(lo, 64), h = rounded(hi, 64);
            assert(l.value.sig == h.value.sig && l.value.se == h.value.se && l.flags == h.flags);
            set(reference, l.value);
            result p53 = rounded(reference, 53), p24 = rounded(reference, 24);
            printf("%u %016" PRIx64 " %04x %016" PRIx64 " %04x %016" PRIx64 " %04x %04x %016" PRIx64 " %04x %04x %016" PRIx64 " %04x %04x\n",
                   op, a.sig, a.se, b.sig, b.se, l.value.sig, l.value.se, l.flags,
                   p53.value.sig, p53.value.se, p53.flags, p24.value.sig, p24.value.se, p24.flags);
        }
    }
    mpfr_clears(y, x, lo, hi, reference, (mpfr_ptr)0);
    return 0;
}
