ENTRY(_start)

MEMORY
{
    /* Top 128 KiB of D1 AXI SRAM, reserved by the kernel's memory.x. */
    APP (rwx) : ORIGIN = 0x24060000, LENGTH = 128K
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
        KEEP(*(.text._start))
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

    /* The loader sets PSP and initializes the PT_LOAD segments before entry. */
    __user_stack_top = ORIGIN(APP) + LENGTH(APP);
    __user_stack_bottom = __user_stack_top - 4K;
    ASSERT(ADDR(.bss) + SIZEOF(.bss) <= __user_stack_bottom,
           "user image overlaps the reserved stack")
}
