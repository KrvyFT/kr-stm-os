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
    beq     .Luser_svc
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

.Luser_svc:
    mrs     r0, PSP
    ldr     r2, [r0, #24]
    subs    r2, #2
    ldrh    r2, [r2]
    movw    r1, #0xdf01
    cmp     r2, r1
    beq     .Lexit_app
    movw    r1, #0xdf02
    cmp     r2, r1
    beq     .Lwrite_app
    b       .Lunsupported_svc

.Lexit_app:
    ldr     r0, [r0]

.Lfinish_app:
    @ MSP points to the suspended kernel context. r0 is the run_app result.
    ldmia   sp!, {r3-r11, lr}
    str     r0, [sp]

    movs    r1, #0
    msr     CONTROL, r1
    isb
    bx      lr

.Lwrite_app:
    push    {r4, lr}
    mov     r4, r0
    ldr     r0, [r4]
    ldr     r1, [r4, #4]
    bl      __kr_sys_write
    str     r0, [r4]
    ldmia   sp!, {r4, lr}
    bx      lr

.Lunsupported_svc:
    udf     #0
.size SVCall, . - SVCall

.p2align 2
.global UsageFault
.type UsageFault,%function
.thumb_func
UsageFault:
    b       .Lapp_fault
.size UsageFault, . - UsageFault

.p2align 2
.global MemoryManagement
.type MemoryManagement,%function
.thumb_func
MemoryManagement:
    b       .Lapp_fault
.size MemoryManagement, . - MemoryManagement

.Lapp_fault:
    @ Only Thread mode with a basic PSP frame can use the app exit path.
    mvn.w   r1, #2
    cmp     lr, r1
    bne     .Lfatal_fault

    @ Rust installs this callback before enabling the configurable faults.
    ldr     r3, =__kr_app_fault_hook
    ldr     r3, [r3]
    cbz     r3, .Lfatal_fault
    mrs     r0, PSP
    mrs     r2, IPSR
    push    {r4, lr}
    blx     r3
    ldmia   sp!, {r4, lr}

    cbz     r0, .Lfatal_fault
    movs    r0, #0
    b       .Lfinish_app

.Lfatal_fault:
    b       DefaultHandler

.ltorg

.section .bss.app_fault_hook,"aw",%nobits
.p2align 2
.global __kr_app_fault_hook
.type __kr_app_fault_hook,%object
__kr_app_fault_hook:
    .zero 4
.size __kr_app_fault_hook, . - __kr_app_fault_hook
