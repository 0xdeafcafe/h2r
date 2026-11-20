/* Runs protocol.h records through the host's real SSE unit. x86_64 only; on
 * Apple silicon build with `clang -arch x86_64` and run under Rosetta
 * (`arch -x86_64`). Used only to record tests/fixtures and for opt-in spot
 * checks; normal tests never run it.
 *
 *   sse_harness        binary records on stdin, results on stdout
 *   sse_harness -t     text: "op pred mxcsr a b" in hex per line */
#if defined(__linux__) && !defined(_GNU_SOURCE)
// glibc exposes sigaction and the named ucontext register fields under C11
// with feature-test macros set before the first system header.
#define _GNU_SOURCE
#endif
#if !defined(__x86_64__)
#error "sse_harness.c runs the host's SSE unit and needs x86_64"
#endif
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ucontext.h>

#include "protocol.h"

__extension__ typedef unsigned __int128 u128;
typedef float v4 __attribute__((vector_size(16)));

static sigjmp_buf jb;
static volatile uint32_t fault_mxcsr;

static void on_fpe(int sig, siginfo_t *si, void *ctx) {
    (void)sig;
    (void)si;
    ucontext_t *uc = (ucontext_t *)ctx;
#if defined(__APPLE__)
    fault_mxcsr = uc->uc_mcontext->__fs.__fpu_mxcsr;
#else
    fault_mxcsr = uc->uc_mcontext.fpregs->mxcsr;
#endif
    siglongjmp(jb, 1);
}

static v4 tov(u128 x) { v4 v; memcpy(&v, &x, 16); return v; }
static u128 fromv(v4 v) { u128 x; memcpy(&x, &v, 16); return x; }

#define BIN(insn)                                                              \
    __asm__ volatile("ldmxcsr %2\n\t" insn " %1, %0\n\tstmxcsr %3"             \
                     : "+x"(va) : "x"(vb), "m"(mx), "m"(mxo) : "memory");                 \
    res = fromv(va);

#define CMP(insn, k)                                                           \
    case k:                                                                    \
        __asm__ volatile("ldmxcsr %2\n\t" insn " $" #k ", %1, %0\n\tstmxcsr %3" \
                         : "+x"(va) : "x"(vb), "m"(mx), "m"(mxo) : "memory");             \
        break;

#define CMPS(insn)                                                             \
    switch (pred & 7) {                                                        \
        CMP(insn, 0) CMP(insn, 1) CMP(insn, 2) CMP(insn, 3)                    \
        CMP(insn, 4) CMP(insn, 5) CMP(insn, 6) CMP(insn, 7)                    \
    }                                                                          \
    res = fromv(va);

static int run(int op, int pred, uint32_t mxcsr, u128 a, u128 b, uint32_t *mout, u128 *rout) {
    volatile uint32_t mx = mxcsr, mxo = 0;
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sigemptyset(&sa.sa_mask);
    sa.sa_sigaction = on_fpe;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGFPE, &sa, NULL);
    if (sigsetjmp(jb, 1)) {
        uint32_t clean = 0x1f80;
        __asm__ volatile("ldmxcsr %0" ::"m"(clean));
        *mout = fault_mxcsr;
        *rout = 0;
        return 1;
    }
    v4 va = tov(a), vb = tov(b);
    u128 res = 0;
    switch (op) {
    case OP_ADDSS: BIN("addss") break;
    case OP_ADDPS: BIN("addps") break;
    case OP_SUBSS: BIN("subss") break;
    case OP_SUBPS: BIN("subps") break;
    case OP_MULSS: BIN("mulss") break;
    case OP_MULPS: BIN("mulps") break;
    case OP_DIVSS: BIN("divss") break;
    case OP_DIVPS: BIN("divps") break;
    case OP_SQRTSS: BIN("sqrtss") break;
    case OP_SQRTPS: BIN("sqrtps") break;
    case OP_MINSS: BIN("minss") break;
    case OP_MINPS: BIN("minps") break;
    case OP_MAXSS: BIN("maxss") break;
    case OP_MAXPS: BIN("maxps") break;
    case OP_RCPSS: BIN("rcpss") break;
    case OP_RCPPS: BIN("rcpps") break;
    case OP_RSQRTSS: BIN("rsqrtss") break;
    case OP_RSQRTPS: BIN("rsqrtps") break;
    case OP_CMPSS: CMPS("cmpss") break;
    case OP_CMPPS: CMPS("cmpps") break;
    case OP_COMISS:
    case OP_UCOMISS: {
        uint64_t fl;
        if (op == OP_COMISS)
            __asm__ volatile("ldmxcsr %3\n\tcomiss %2, %0\n\tpushfq\n\tpopq %1\n\tstmxcsr %4"
                             : "+x"(va), "=r"(fl) : "x"(vb), "m"(mx), "m"(mxo) : "cc", "memory");
        else
            __asm__ volatile("ldmxcsr %3\n\tucomiss %2, %0\n\tpushfq\n\tpopq %1\n\tstmxcsr %4"
                             : "+x"(va), "=r"(fl) : "x"(vb), "m"(mx), "m"(mxo) : "cc", "memory");
        res = (u128)(fl & 0x45);
        break;
    }
    case OP_CVTSI2SS: {
        uint32_t i = (uint32_t)b;
        __asm__ volatile("ldmxcsr %2\n\tcvtsi2ssl %1, %0\n\tstmxcsr %3"
                         : "+x"(va) : "m"(i), "m"(mx), "m"(mxo) : "memory");
        res = fromv(va);
        break;
    }
    case OP_CVTPI2PS: {
        uint64_t m = (uint64_t)b;
        __asm__ volatile("ldmxcsr %2\n\tcvtpi2ps %1, %0\n\tstmxcsr %3"
                         : "+x"(va) : "m"(m), "m"(mx), "m"(mxo) : "memory");
        res = fromv(va);
        break;
    }
    case OP_CVTSS2SI:
    case OP_CVTTSS2SI: {
        uint32_t r;
        if (op == OP_CVTSS2SI)
            __asm__ volatile("ldmxcsr %2\n\tcvtss2si %1, %0\n\tstmxcsr %3"
                             : "=r"(r) : "x"(vb), "m"(mx), "m"(mxo) : "memory");
        else
            __asm__ volatile("ldmxcsr %2\n\tcvttss2si %1, %0\n\tstmxcsr %3"
                             : "=r"(r) : "x"(vb), "m"(mx), "m"(mxo) : "memory");
        res = r;
        break;
    }
    case OP_CVTPS2PI:
    case OP_CVTTPS2PI: {
        uint64_t r;
        if (op == OP_CVTPS2PI)
            __asm__ volatile("ldmxcsr %2\n\tcvtps2pi %1, %%mm0\n\tstmxcsr %3\n\tmovq %%mm0, %0\n\temms"
                             : "=m"(r) : "x"(vb), "m"(mx), "m"(mxo) : "mm0", "memory");
        else
            __asm__ volatile("ldmxcsr %2\n\tcvttps2pi %1, %%mm0\n\tstmxcsr %3\n\tmovq %%mm0, %0\n\temms"
                             : "=m"(r) : "x"(vb), "m"(mx), "m"(mxo) : "mm0", "memory");
        res = r;
        break;
    }
    default:
        fprintf(stderr, "bad op %d\n", op);
        exit(2);
    }
    {
        uint32_t clean = 0x1f80;
        __asm__ volatile("ldmxcsr %0" ::"m"(clean));
    }
    *mout = mxo;
    *rout = res;
    return 0;
}

static u128 parse128(const char *s) {
    u128 x = 0;
    for (; *s; s++) {
        int c = *s, d;
        if (c >= '0' && c <= '9') d = c - '0';
        else if (c >= 'a' && c <= 'f') d = c - 'a' + 10;
        else if (c >= 'A' && c <= 'F') d = c - 'A' + 10;
        else continue;
        x = (x << 4) | (u128)d;
    }
    return x;
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "-t")) {
        char line[512];
        while (fgets(line, sizeof line, stdin)) {
            unsigned op, pred, mx;
            char as[64], bs[64];
            if (sscanf(line, "%u %u %x %63s %63s", &op, &pred, &mx, as, bs) != 5) continue;
            uint32_t mo;
            u128 r;
            int st = run((int)op, (int)pred, mx, parse128(as), parse128(bs), &mo, &r);
            printf("%s mxcsr=%04x r=%016llx%016llx\n", st ? "XM" : "ok", mo,
                   (unsigned long long)(uint64_t)(r >> 64), (unsigned long long)(uint64_t)r);
        }
        return 0;
    }
    unsigned char in[IN_SIZE], out[OUT_SIZE];
    while (fread(in, 1, IN_SIZE, stdin) == IN_SIZE) {
        uint32_t mx, mo;
        u128 a, b, r;
        memcpy(&mx, in + 4, 4);
        memcpy(&a, in + 8, 16);
        memcpy(&b, in + 24, 16);
        int st = run(in[0], in[1], mx, a, b, &mo, &r);
        memset(out, 0, sizeof out);
        out[0] = (unsigned char)st;
        memcpy(out + 4, &mo, 4);
        memcpy(out + 8, &r, 16);
        fwrite(out, 1, OUT_SIZE, stdout);
    }
    return 0;
}
