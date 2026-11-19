.text
#if defined(__ELF__)
.globl execute
.type execute, @function
execute:
#else
.globl _execute
_execute:
#endif
    pushq %rbp
    pushq %rbx
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    subq $8, %rsp
    movq %rdi, %r13
    movq %rsi, %r12
    movups 0(%r13), %xmm0
    movups 16(%r13), %xmm1
    movups 32(%r13), %xmm2
    movups 48(%r13), %xmm3
    movups 64(%r13), %xmm4
    movups 80(%r13), %xmm5
    movups 96(%r13), %xmm6
    movups 112(%r13), %xmm7
    movq 128(%r13), %mm0
    movq 136(%r13), %mm1
    movq 144(%r13), %mm2
    movq 152(%r13), %mm3
    movq 160(%r13), %mm4
    movq 168(%r13), %mm5
    movq 176(%r13), %mm6
    movq 184(%r13), %mm7
    ldmxcsr 192(%r13)
    movl 200(%r13), %eax
    movl 204(%r13), %ecx
    leaq 208(%r13), %rdi
    movl 196(%r13), %r14d
    pushq %r14
    popfq
    callq *%r12
    pushfq
    popq %r14
    movl %r14d, 196(%r13)
    sfence // Complete non-temporal memory stores before reporting memory.
    stmxcsr 192(%r13)
    movl %eax, 200(%r13)
    movl %ecx, 204(%r13)
    movups %xmm0, 0(%r13)
    movups %xmm1, 16(%r13)
    movups %xmm2, 32(%r13)
    movups %xmm3, 48(%r13)
    movups %xmm4, 64(%r13)
    movups %xmm5, 80(%r13)
    movups %xmm6, 96(%r13)
    movups %xmm7, 112(%r13)
    movq %mm0, 128(%r13)
    movq %mm1, 136(%r13)
    movq %mm2, 144(%r13)
    movq %mm3, 152(%r13)
    movq %mm4, 160(%r13)
    movq %mm5, 168(%r13)
    movq %mm6, 176(%r13)
    movq %mm7, 184(%r13)
    emms
    addq $8, %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbx
    popq %rbp
    retq

#if defined(__ELF__)
.size execute, . - execute
.section .note.GNU-stack,"",@progbits
#endif
