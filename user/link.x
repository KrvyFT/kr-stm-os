ENTRY(_start)

MEMORY
{
    /* Top 128 KiB of D1 AXI SRAM, reserved by the kernel's memory.x. */
    APP   (rwx) : ORIGIN = 0x24060000, LENGTH = 124K
    STACK (rw)  : ORIGIN = 0x2407F000, LENGTH = 4K
}

PHDRS
{
    code PT_LOAD FLAGS(5); /* Read and execute */
    data PT_LOAD FLAGS(6); /* Read and write */
}

SECTIONS
{
    .text ORIGIN(APP) :
    {
        KEEP(*(.text.entry))
        *(.text .text.*)
    } > APP :code

    .rodata :
    {
        *(.rodata .rodata.*)
    } > APP :code

    .ARM.extab :
    {
        *(.ARM.extab .ARM.extab.*)
    } > APP :code

    .ARM.exidx :
    {
        *(.ARM.exidx .ARM.exidx.*)
    } > APP :code

    .data : ALIGN(8)
    {
        *(.data .data.*)
    } > APP :data

    .bss (NOLOAD) : ALIGN(8)
    {
        *(.bss .bss.*)
        *(COMMON)
    } > APP :data

    /* The kernel initializes the PT_LOAD segments and sets PSP before entry. */
    __user_stack_bottom = ORIGIN(STACK);
    __user_stack_top = ORIGIN(STACK) + LENGTH(STACK);
    ASSERT((_start & ~1) == ORIGIN(APP), "user entry must start at APP origin")
}
