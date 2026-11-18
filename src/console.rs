use core::{
    cell::RefCell,
    fmt::{self, Write},
    sync::atomic::{AtomicBool, Ordering},
};

use cortex_m::interrupt::{self, Mutex};
use heapless::{Deque, String};
use stm32h7xx_hal::{
    dma::dma::Stream0,
    hal::serial::Write as _,
    pac::{DMA1, DMAMUX1, USART1},
    serial::Tx,
};

static TX_QUEUE: Mutex<RefCell<Deque<u8, 1024>>> = Mutex::new(RefCell::new(Deque::new()));
static PANIC_DMA_READY: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub enum WriteError {
    TooLong,
    Full,
}

pub fn try_print(s: &str) -> Result<(), WriteError> {
    let mut message = String::<128>::new();
    message.write_str(s).map_err(|_| WriteError::TooLong)?;
    interrupt::free(|cs| {
        let mut queue = TX_QUEUE.borrow(cs).borrow_mut();

        if queue.capacity() - queue.len() < message.len() {
            return Err(WriteError::Full);
        }

        for byte in message.bytes() {
            let _ = queue.push_back(byte);
        }
        Ok(())
    })
}

pub struct ConsoleDriver {
    tx: Tx<USART1>,
    _panic_dma: Stream0<DMA1>,
    pending: Option<u8>,
}

impl ConsoleDriver {
    pub fn new(tx: Tx<USART1>, panic_dma: Stream0<DMA1>) -> Self {
        PANIC_DMA_READY.store(true, Ordering::Release);
        Self {
            tx,
            _panic_dma: panic_dma,
            pending: None,
        }
    }

    pub fn poll(&mut self) {
        if self.pending.is_none() {
            self.pending = interrupt::free(|cs| TX_QUEUE.borrow(cs).borrow_mut().pop_front())
        }

        if let Some(byte) = self.pending {
            if self.tx.write(byte).is_ok() {
                self.pending = None;
            }
        }
    }
}

pub fn write_panic(args: fmt::Arguments<'_>) {
    cortex_m::interrupt::disable();
    if !PANIC_DMA_READY.swap(false, Ordering::AcqRel) {
        return;
    }

    // The atomic flag gives this fatal path exclusive access to the buffer
    // and the reserved stream, including if formatting itself panics.
    let buffer = unsafe { &mut *(&raw mut PANIC_BUFFER) };
    let _ = buffer.write_fmt(args);
    if buffer.len == 0 {
        return;
    }
    let address = buffer.bytes.as_ptr() as usize;
    let len = buffer.len;

    if cortex_m::peripheral::SCB::dcache_enabled() {
        // Foreground execution has stopped and maskable interrupts are off.
        let mut scb = unsafe { cortex_m::Peripherals::steal() }.SCB;
        scb.clean_dcache_by_address(address, len);
    }
    cortex_m::asm::dsb();

    // Normal output uses the USART in polling mode. Stream 0 is reserved
    // by ConsoleDriver and the fatal handler is the only DMA user of USART1.
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

    let _ = wait_for_usart(|| {
        let flags = dma.lisr.read();
        flags.tcif0().bit_is_set()
            || flags.teif0().bit_is_set()
            || flags.dmeif0().bit_is_set()
            || flags.feif0().bit_is_set()
    });
    usart.cr3.modify(|_, w| w.dmat().clear_bit());
    stream.cr.modify(|_, w| w.en().clear_bit());
    if dma.lisr.read().tcif0().bit_is_set() {
        let _ = wait_for_usart(|| usart.isr.read().tc().bit_is_set());
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

fn wait_for_usart(ready: impl Fn() -> bool) -> fmt::Result {
    for _ in 0..16_000_000 {
        if ready() {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(fmt::Error)
}
