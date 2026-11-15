// A persistent, bit-only binary protocol; no C floating-point operations.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <setjmp.h>
#include <sys/mman.h>
#include <unistd.h>
#include <libkern/OSCacheControl.h>

typedef struct __attribute__((aligned(16))) {
    uint32_t xmm[8][4];
    uint32_t mm[8][2];
    uint32_t mxcsr, eflags, eax, ecx;
    uint32_t memory[4];
} State;
_Static_assert(sizeof(State) == 224, "wire layout");
extern void execute(State *, void *);
static sigjmp_buf recovery;
static volatile sig_atomic_t active, fault;
static void trapped(int signo) {
    if (!active) _exit(128 + signo);
    fault = signo;
    siglongjmp(recovery, 1);
}
static int read_exact(void *p, size_t n) {
    return fread(p, 1, n, stdin) == n;
}
int main(void) {
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = trapped;
    sigemptyset(&action.sa_mask);
    int signals[] = { SIGILL, SIGFPE, SIGBUS, SIGSEGV };
    for (unsigned i = 0; i < sizeof(signals)/sizeof(*signals); i++)
        if (sigaction(signals[i], &action, NULL)) return 2;
    size_t size = (size_t)getpagesize();
    void *page = mmap(NULL, size, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANON, -1, 0);
    if (page == MAP_FAILED) return 3;
    setvbuf(stdout, NULL, _IONBF, 0);
    uint8_t previous[16] = {0};
    uint32_t previous_len = 0;
    for (;;) {
        uint32_t len;
        uint8_t bytes[16];
        State state;
        size_t header = fread(&len, 1, sizeof(len), stdin);
        if (!header && feof(stdin)) break;
        if (header != sizeof(len)) return 4;
        if (!read_exact(bytes, sizeof(bytes)) || !read_exact(&state, sizeof(state))) return 4;
        if (!len || len > 15 || (state.mxcsr & ~0xffbfu) || (state.mxcsr & 0x1f80u) != 0x1f80u) return 5;
        // Only arithmetic EFLAGS are supplied; DF/TF/privileged bits are excluded.
        state.eflags = (state.eflags & 0x8d5u) | 0x202u;
        if (len != previous_len || memcmp(bytes, previous, len)) {
            if (mprotect(page, size, PROT_READ | PROT_WRITE)) return 6;
            memcpy(page, bytes, len);
            ((uint8_t *)page)[len] = 0xc3; // RET preserves flags and MXCSR.
            sys_icache_invalidate(page, size);
            if (mprotect(page, size, PROT_READ | PROT_EXEC)) return 7;
            memcpy(previous, bytes, len);
            previous_len = len;
        }
        uint32_t host_mxcsr;
        __asm__ volatile("stmxcsr %0" : "=m"(host_mxcsr));
        fault = 0;
        if (sigsetjmp(recovery, 1) == 0) {
            active = 1;
            execute(&state, page);
        }
        active = 0;
        __asm__ volatile("emms; ldmxcsr %0" :: "m"(host_mxcsr));
        // Automatic variables modified after sigsetjmp are indeterminate on
        // siglongjmp. The API reports no state on a fault; send initialized bytes.
        if (fault) memset(&state, 0, sizeof(state));
        uint32_t signo = (uint32_t)fault;
        if (fwrite(&state, 1, sizeof(state), stdout) != sizeof(state) ||
            fwrite(&signo, 1, sizeof(signo), stdout) != sizeof(signo)) return 8;
    }
    munmap(page, size);
    return ferror(stdin) ? 9 : 0;
}
