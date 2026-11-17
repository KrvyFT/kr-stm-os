#![no_std]
#![no_main]

mod console;
mod lang_items;

use core::fmt::Write;
use cortex_m_rt::entry;
use stm32h7xx_hal::time::U32Ext;
use stm32h7xx_hal::{self as _, gpio::GpioExt, pac, pwr::PwrExt, rcc::RccExt, serial::SerialExt};

use crate::console::SerialConsole;

#[entry]
fn main() -> ! {
    let cp = cortex_m::Peripherals::take().unwrap();
    let dp = pac::Peripherals::take().unwrap();

    let power = dp.PWR.constrain().freeze();
    let ccdr = dp.RCC.constrain().freeze(power, &dp.SYSCFG);

    let gpioa = dp.GPIOA.split(ccdr.peripheral.GPIOA);
    let (rx, tx) = (gpioa.pa10.into_alternate(), gpioa.pa9.into_alternate());

    let serial = dp
        .USART1
        .serial((tx, rx), 19_200.bps(), ccdr.peripheral.USART1, &ccdr.clocks)
        .unwrap();

    let (tx, rx) = serial.split();
    console::init_console(SerialConsole::new(tx, rx));
    loop {}
}
