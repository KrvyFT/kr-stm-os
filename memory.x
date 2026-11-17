MEMORY
{
    /* STM32H743VGT6: use Flash bank 1 and D1 AXI SRAM. */
    FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 512K
    RAM   (rwx) : ORIGIN = 0x24000000, LENGTH = 512K
}
