MEMORY
{
    /* STM32H743VGT6: use Flash bank 1 and the first 384 KiB of D1 AXI SRAM. */
    FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 512K
    /* The remaining 128 KiB starts at 0x24060000 (see ../user/link.x). */
    RAM   (rwx) : ORIGIN = 0x24000000, LENGTH = 384K
}
