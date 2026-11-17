use core::{
    cell::RefCell,
    fmt::Write,
    mem,
    ops::{Deref, DerefMut},
};

use cortex_m::interrupt::{self, Mutex};
use nb::block;
use stm32h7xx_hal::{
    hal::serial::Read,
    pac::USART1,
    serial::{Rx, Tx},
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

pub type Usart1Console = SerialConsole<Tx<USART1>, Rx<USART1>>;

enum ConsoleState {
    Uninitialized,
    Available(Usart1Console),
    InUse,
}

static CONSOLE: Mutex<RefCell<ConsoleState>> =
    Mutex::new(RefCell::new(ConsoleState::Uninitialized));

pub fn init_console(console: Usart1Console) {
    let initialized = interrupt::free(|cs| {
        let mut slot = CONSOLE.borrow(cs).borrow_mut();
        if !matches!(*slot, ConsoleState::Uninitialized) {
            return false;
        }
        *slot = ConsoleState::Available(console);
        true
    });
    assert!(initialized, "console already initialized");
}

/// Acquires the console until the returned guard is dropped.
/// Returns `None` if initialization is pending or another caller holds it.
pub fn get_console() -> Option<ConsoleGuard> {
    interrupt::free(|cs| {
        let mut slot = CONSOLE.borrow(cs).borrow_mut();
        match mem::replace(&mut *slot, ConsoleState::InUse) {
            ConsoleState::Available(console) => Some(ConsoleGuard {
                console: Some(console),
            }),
            state => {
                *slot = state;
                None
            }
        }
    })
}

pub struct ConsoleGuard {
    console: Option<Usart1Console>,
}

impl Deref for ConsoleGuard {
    type Target = Usart1Console;

    fn deref(&self) -> &Self::Target {
        self.console.as_ref().unwrap()
    }
}

impl DerefMut for ConsoleGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.console.as_mut().unwrap()
    }
}

impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        if let Some(console) = self.console.take() {
            interrupt::free(|cs| {
                *CONSOLE.borrow(cs).borrow_mut() = ConsoleState::Available(console);
            });
        }
    }
}
