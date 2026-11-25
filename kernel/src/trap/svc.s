.syntax unified
.thumb
.section .text.batch_trap,"ax",%progbits

.p2align 2
.global __kr_run_app
.type __kr_run_app,%function
.thumb_func
__kr_run_app:
    @ The C ABI carries the initial PSP and final exit code in r0.
    dsb
    svc     #0
.global __kr_resume_kernel
__kr_resume_kernel:
    bx      lr
.size __kr_run_app, . - __kr_run_app

.p2align 2
.global SVCall
.type SVCall,%function
.thumb_func
SVCall:
    @ Capture MSP before pushing. BL overwrites the EXC_RETURN in LR.
    mrs     r0, MSP
    mov     r1, lr
    mrs     r2, IPSR
    push    {{r3, lr}}
    bl      {dispatch}
    ldmia   sp!, {{r3, lr}}

    cmp     r0, #{resume}
    beq     .Lexception_return
    cmp     r0, #{start}
    beq     .Lstart_app
    cmp     r0, #{finish}
    beq     .Lfinish_app
    b       {default_handler}

.Lstart_app:
    @ Save r4-r11, which the basic hardware frame leaves live. r3 pads
    @ the software save area to 40 bytes and keeps MSP 8-byte aligned.
    stmdb   sp!, {{r3-r11, lr}}
    movs    r0, #0
    mov     r4, r0
    mov     r5, r0
    mov     r6, r0
    mov     r7, r0
    mov     r8, r0
    mov     r9, r0
    mov     r10, r0
    mov     r11, r0
    movs    r0, #1
    msr     CONTROL, r0
    isb
    mvn.w   lr, #{psp_basic_inverted}
    bx      lr

.Lfinish_app:
    @ The Rust dispatcher has already set r0 in the kernel hardware frame.
    ldmia   sp!, {{r3-r11, lr}}
    movs    r0, #0
    msr     CONTROL, r0
    isb
.Lexception_return:
    bx      lr
.size SVCall, . - SVCall

@ All three vectors enter the same code. IPSR distinguishes their causes.
.global UsageFault
.thumb_set UsageFault, SVCall
.global MemoryManagement
.thumb_set MemoryManagement, SVCall
