/* Runs protocol.h records through h2simd.h: binary records on stdin, results
 * on stdout. Built and driven by tests/differential.rs (Rust side vs C side)
 * and tests/hardware.rs (C side vs recorded SSE results). Any host. */
#include <stdio.h>
#include <string.h>

#include "h2simd.h"
#include "protocol.h"

static uint32_t run(int op, uint8_t pred, uint32_t *mx, h2u128 a, h2u128 b, h2u128 *r) {
    uint32_t b32 = (uint32_t)b, a32 = (uint32_t)a, o32 = 0xdeadbeefu;
    uint64_t o64 = 0xdeadbeefdeadbeefull;
    uint32_t st;
    switch (op) {
    case OP_ADDSS: return h2s_addss(mx, a, b32, r);
    case OP_ADDPS: return h2s_addps(mx, a, b, r);
    case OP_SUBSS: return h2s_subss(mx, a, b32, r);
    case OP_SUBPS: return h2s_subps(mx, a, b, r);
    case OP_MULSS: return h2s_mulss(mx, a, b32, r);
    case OP_MULPS: return h2s_mulps(mx, a, b, r);
    case OP_DIVSS: return h2s_divss(mx, a, b32, r);
    case OP_DIVPS: return h2s_divps(mx, a, b, r);
    case OP_SQRTSS: return h2s_sqrtss(mx, a, b32, r);
    case OP_SQRTPS: return h2s_sqrtps(mx, b, r);
    case OP_MINSS: return h2s_minss(mx, a, b32, r);
    case OP_MINPS: return h2s_minps(mx, a, b, r);
    case OP_MAXSS: return h2s_maxss(mx, a, b32, r);
    case OP_MAXPS: return h2s_maxps(mx, a, b, r);
    case OP_CMPSS: return h2s_cmpss(mx, a, b32, pred, r);
    case OP_CMPPS: return h2s_cmpps(mx, a, b, pred, r);
    case OP_COMISS: st = h2s_comiss(mx, a32, b32, &o32); if (!st) *r = o32; else if (o32 != 0xdeadbeefu) return 3; return st;
    case OP_UCOMISS: st = h2s_ucomiss(mx, a32, b32, &o32); if (!st) *r = o32; else if (o32 != 0xdeadbeefu) return 3; return st;
    case OP_CVTSI2SS: return h2s_cvtsi2ss(mx, a, b32, r);
    case OP_CVTSS2SI: st = h2s_cvtss2si(mx, b32, &o32); if (!st) *r = o32; else if (o32 != 0xdeadbeefu) return 3; return st;
    case OP_CVTTSS2SI: st = h2s_cvttss2si(mx, b32, &o32); if (!st) *r = o32; else if (o32 != 0xdeadbeefu) return 3; return st;
    case OP_CVTPI2PS: return h2s_cvtpi2ps(mx, a, (uint64_t)b, r);
    case OP_CVTPS2PI: st = h2s_cvtps2pi(mx, (uint64_t)b, &o64); if (!st) *r = o64; else if (o64 != 0xdeadbeefdeadbeefull) return 3; return st;
    case OP_CVTTPS2PI: st = h2s_cvttps2pi(mx, (uint64_t)b, &o64); if (!st) *r = o64; else if (o64 != 0xdeadbeefdeadbeefull) return 3; return st;
    case OP_RCPSS: *r = h2s_rcpss(a, b32); return 0;
    case OP_RCPPS: *r = h2s_rcpps(b); return 0;
    case OP_RSQRTSS: *r = h2s_rsqrtss(a, b32); return 0;
    case OP_RSQRTPS: *r = h2s_rsqrtps(b); return 0;
    default: return 2;
    }
}

int main(void) {
    unsigned char in[IN_SIZE], out[OUT_SIZE];
    static unsigned char inbuf[1 << 16], outbuf[1 << 16];
    setvbuf(stdin, (char *)inbuf, _IOFBF, sizeof inbuf);
    setvbuf(stdout, (char *)outbuf, _IOFBF, sizeof outbuf);
    while (fread(in, 1, IN_SIZE, stdin) == IN_SIZE) {
        uint32_t mx;
        h2u128 a, b, r = 0;
        /* Poison: a result the call does not write must stay invisible. */
        h2u128 poison = ((h2u128)0xdeadbeefdeadbeefull << 64) | 0xdeadbeefdeadbeefull;
        memcpy(&mx, in + 4, 4);
        memcpy(&a, in + 8, 16);
        memcpy(&b, in + 24, 16);
        r = poison;
        uint32_t st = run(in[0], in[1], &mx, a, b, &r);
        if (st >= 2) return (int)st;
        if (st == 1) {
            if (r != poison) return 3; /* wrote the destination on #XM */
            r = 0;
        }
        memset(out, 0, sizeof out);
        out[0] = (unsigned char)st;
        memcpy(out + 4, &mx, 4);
        memcpy(out + 8, &r, 16);
        fwrite(out, 1, OUT_SIZE, stdout);
    }
    return 0;
}
