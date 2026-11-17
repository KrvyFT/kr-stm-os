use core::{cell::RefCell, fmt::Write};

use cortex_m::interrupt::{self, Mutex};
use nb::block;
use stm32h7xx_hal::{
    hal::serial::Read,
    pac::USART1,
    serial::{Rx, Serial, Tx},
};

pub struct SerialConsole<Tx, Rx> {
    tx: Tx,
    rx: Rx,
}

impl<Tx: Write, Rx: Read<u8>> SerialConsole<Tx, Rx> {
    pub fn new(tx: Tx, rx: Rx) -> Self {
        Self { tx, rx }
    }

    pub fn try_read_byte(&mut self) -> nb::Result<u8, Rx::Error> {
        self.rx.read()
    }

    pub fn read_byte(&mut self) -> Result<u8, Rx::Error> {
        block!(self.try_read_byte())
    }
}

impl<Tx: Write, Rx> Write for SerialConsole<Tx, Rx> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.tx.write_str(s)
    }
}

type Usart1Console = SerialConsole<Tx<USART1>, Rx<USART1>>;

static CONSOLE: Mutex<RefCell<Option<Usart1Console>>> = Mutex::new(RefCell::new(None));

pub fn init_console(console: Usart1Console) {
    let initialized = interrupt::free(|cs| {
        let mut slot = CONSOLE.borrow(cs).borrow_mut();
        if slot.is_some() {
            return false;
        }
        *slot = Some(console);
        true
    });
    assert!(initialized, "console already initialized");
}

pub fn with_console<R>(f: impl FnOnce(&mut Usart1Console) -> R) -> Option<R> {
    interrupt::free(|cs| CONSOLE.borrow(cs).borrow_mut().as_mut().map(f))
}
