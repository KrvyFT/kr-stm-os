.syntax unified
.thumb

.section .text._start, "ax", %progbits
.global _start
.type _start, %function
.thumb_func
_start:
    /* Preserve the caller's LR and return user_main's result in r0. */
    b.w user_main
.size _start, . - _start
