/* FNV-1a of h2s_rcp_approx and h2s_rsqrt_approx over every input, for
 * tests/approx.rs to compare with the hashes of measured Intel hardware. */
#include <inttypes.h>
#include <stdio.h>

#include "h2simd.h"

int main(void) {
    uint64_t r = 1469598103934665603ull, s = r; /* the recording's offset basis */
    uint64_t x;
    for (x = 0; x <= 0xffffffffull; x++) {
        r = (r ^ h2s_rcp_approx((uint32_t)x)) * 0x100000001b3ull;
        s = (s ^ h2s_rsqrt_approx((uint32_t)x)) * 0x100000001b3ull;
    }
    printf("%016" PRIx64 " %016" PRIx64 "\n", r, s);
    return 0;
}
