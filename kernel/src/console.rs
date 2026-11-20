mod panic_dma;

use core::cell::RefCell;

use cortex_m::interrupt::{self, Mutex};
use heapless::Deque;
use stm32h7xx_hal::{
    dma::dma::Stream0,
    hal::serial::Write as _,
    pac::{DMA1, USART1},
    serial::Tx,
};

pub(crate) use panic_dma::write as write_panic;

const TX_QUEUE_CAPACITY: usize = 1024;
const MAX_MESSAGE_BYTES: usize = 128;
static TX_QUEUE: Mutex<RefCell<Deque<u8, TX_QUEUE_CAPACITY>>> =
    Mutex::new(RefCell::new(Deque::new()));

#[derive(Debug)]
pub enum WriteError {
    TooLong,
    Full,
}

/// Enqueues the entire UTF-8 message, or leaves the queue untouched on error.
pub fn try_print(message: &str) -> Result<(), WriteError> {
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(WriteError::TooLong);
    }

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
    // Reserve this stream for panic_dma for the lifetime of the driver.
    _panic_dma: Stream0<DMA1>,
    pending: Option<u8>,
}

impl ConsoleDriver {
    pub fn new(tx: Tx<USART1>, panic_dma: Stream0<DMA1>) -> Self {
        panic_dma::arm();
        Self {
            tx,
            _panic_dma: panic_dma,
            pending: None,
        }
    }

    /// Attempts to write one byte, retaining it while USART1 is busy.
    pub fn poll(&mut self) {
        if self.pending.is_none() {
            self.pending = interrupt::free(|cs| TX_QUEUE.borrow(cs).borrow_mut().pop_front());
        }

        if let Some(byte) = self.pending
            && self.tx.write(byte).is_ok()
        {
            self.pending = None;
        }
    }

    /// Drains the queue and waits for the last USART frame to finish.
    pub fn wait_for_write(&mut self) {
        loop {
            self.poll();
            let queue_empty = interrupt::free(|cs| TX_QUEUE.borrow(cs).borrow().is_empty());
            if queue_empty && self.pending.is_none() && self.tx.flush().is_ok() {
                return;
            }
            core::hint::spin_loop();
        }
    }
}
