use core::{
    fmt::{self, Write},
    sync::atomic::{AtomicBool, Ordering},
};

use stm32h7xx_hal::pac::{DMA1, DMAMUX1, USART1};

const POLL_LIMIT: usize = 16_000_000;
static PANIC_DMA_READY: AtomicBool = AtomicBool::new(false);

pub(super) fn arm() {
    PANIC_DMA_READY.store(true, Ordering::Release);
}

pub(crate) fn write(args: fmt::Arguments<'_>) {
    cortex_m::interrupt::disable();
    if !PANIC_DMA_READY.swap(false, Ordering::AcqRel) {
        return;
    }

    let buffer = &raw mut PANIC_BUFFER;
    // SAFETY: Only the first panic after arm() may access this buffer.
    let buffer = unsafe { &mut *buffer };
    let _ = buffer.write_fmt(args);
    if buffer.len == 0 {
        return;
    }
    let address = buffer.bytes.as_ptr() as usize;
    let len = buffer.len;

    if cortex_m::peripheral::SCB::dcache_enabled() {
        // SAFETY: The fatal path never resumes interrupted code, and
        // maskable interrupts are disabled.
        let mut scb = unsafe { cortex_m::Peripherals::steal() }.SCB;
        scb.clean_dcache_by_address(address, len);
    }
    cortex_m::asm::dsb();

    // SAFETY: ConsoleDriver reserves Stream 0. The fatal path has exclusive
    // access to the stream, its DMAMUX channel, and the USART1 transmitter.
    let usart = unsafe { &*USART1::ptr() };
    let dma = unsafe { &*DMA1::ptr() };
    let mux = unsafe { &*DMAMUX1::ptr() };
    let stream = &dma.st[0];
    dma.lifcr.write(|w| {
        w.ctcif0()
            .clear()
            .chtif0()
            .clear()
            .cteif0()
            .clear()
            .cdmeif0()
            .clear()
            .cfeif0()
            .clear()
    });
    mux.ccr[0].write(|w| w.dmareq_id().usart1_tx_dma());
    stream
        .par
        .write(|w| unsafe { w.bits(&raw const usart.tdr as u32) });
    stream.m0ar.write(|w| unsafe { w.bits(address as u32) });
    stream.ndtr.write(|w| w.ndt().bits(len as u16));
    stream.fcr.reset();
    stream
        .cr
        .write(|w| w.dir().memory_to_peripheral().minc().set_bit());
    usart.icr.write(|w| w.tccf().clear());
    stream.cr.modify(|_, w| w.en().set_bit());
    usart.cr3.modify(|_, w| w.dmat().set_bit());

    wait_until(|| {
        let flags = dma.lisr.read();
        if flags.feif0().bit_is_set() {
            // A transient FIFO underrun does not stop the stream.
            dma.lifcr.write(|w| w.cfeif0().clear());
        }
        (flags.tcif0().bit_is_set() && stream.ndtr.read().ndt().bits() == 0)
            || flags.teif0().bit_is_set()
            || flags.dmeif0().bit_is_set()
    });
    // Disabling the stream also sets TCIF0, so check completion first.
    let flags = dma.lisr.read();
    let dma_finished = flags.tcif0().bit_is_set()
        && stream.ndtr.read().ndt().bits() == 0
        && !flags.teif0().bit_is_set()
        && !flags.dmeif0().bit_is_set();
    usart.cr3.modify(|_, w| w.dmat().clear_bit());
    stream.cr.modify(|_, w| w.en().clear_bit());
    if dma_finished {
        wait_until(|| usart.isr.read().tc().bit_is_set());
    }
}

// The linker places .bss in AXI SRAM, which DMA1 can access.
const PANIC_CAPACITY: usize = 512;
#[repr(C, align(32))]
struct PanicBuffer {
    bytes: [u8; PANIC_CAPACITY],
    len: usize,
}

static mut PANIC_BUFFER: PanicBuffer = PanicBuffer {
    bytes: [0; PANIC_CAPACITY],
    len: 0,
};

impl Write for PanicBuffer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let count = text.floor_char_boundary(self.bytes.len() - self.len);
        let end = self.len + count;
        self.bytes[self.len..end].copy_from_slice(&text.as_bytes()[..count]);
        self.len = end;
        if count < text.len() {
            return Err(fmt::Error);
        }
        Ok(())
    }
}

fn wait_until(ready: impl Fn() -> bool) {
    for _ in 0..POLL_LIMIT {
        if ready() {
            return;
        }
        core::hint::spin_loop();
    }
}
