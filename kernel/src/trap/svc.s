.syntax unified
.thumb
.section .text.batch_trap,"ax",%progbits

.p2align 2
.global __kr_run_app
.type __kr_run_app,%function
.thumb_func
__kr_run_app:
    dsb
    svc     #0
.Lresume_kernel:
    bx      lr
.size __kr_run_app, . - __kr_run_app

.p2align 2
.global SVCall
.type SVCall,%function
.thumb_func
SVCall:
    mvn.w   r1, #6
    cmp     lr, r1
    beq     .Lstart_app

    mvn.w   r1, #2
    cmp     lr, r1
    beq     .Lexit_app
    b       .Lunsupported_svc

.Lstart_app:
    ldr     r2, [sp, #24]
    adr.w   r1, .Lresume_kernel
    cmp     r2, r1
    bne     .Lunsupported_svc

    ldr     r0, [sp]
    stmdb   sp!, {r3-r11, lr}
    msr     PSP, r0

    movs    r1, #0
    mov     r4, r1
    mov     r5, r1
    mov     r6, r1
    mov     r7, r1
    mov     r8, r1
    mov     r9, r1
    mov     r10, r1
    mov     r11, r1

    movs    r1, #1
    msr     CONTROL, r1
    isb
    mvn.w   lr, #2
    bx      lr

.Lexit_app:
    mrs     r0, PSP
    ldr     r2, [r0, #24]
    subs    r2, #2
    ldrh    r2, [r2]
    movw    r1, #0xdf01
    cmp     r2, r1
    bne     .Lunsupported_svc

    ldr     r0, [r0]
    ldmia   sp!, {r3-r11, lr}
    str     r0, [sp]

    movs    r1, #0
    msr     CONTROL, r1
    isb
    bx      lr

.Lunsupported_svc:
    udf     #0
.size SVCall, . - SVCall