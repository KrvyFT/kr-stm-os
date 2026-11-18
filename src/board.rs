use stm32h7xx_hal::{
    dma::dma::StreamsTuple,
    gpio::{GpioExt, Output, gpioc::PC13},
    pac,
    pwr::PwrExt,
    rcc::RccExt,
    serial::SerialExt,
    time::U32Ext,
};

use crate::console::ConsoleDriver;

pub(crate) struct Board {
    pub(crate) console: ConsoleDriver,
    pub(crate) led: PC13<Output>,
}

impl Board {
    pub(crate) fn init() -> Self {
        let dp = pac::Peripherals::take().unwrap();

        let power = dp.PWR.constrain().freeze();
        let ccdr = dp.RCC.constrain().freeze(power, &dp.SYSCFG);

        let gpioa = dp.GPIOA.split(ccdr.peripheral.GPIOA);
        let gpioc = dp.GPIOC.split(ccdr.peripheral.GPIOC);
        let led = gpioc.pc13.into_push_pull_output();

        let tx = gpioa.pa9.into_alternate();
        let rx = gpioa.pa10.into_alternate();
        let serial = dp
            .USART1
            .serial((tx, rx), 19_200.bps(), ccdr.peripheral.USART1, &ccdr.clocks)
            .unwrap();

        let (tx, _rx) = serial.split();
        let dma = StreamsTuple::new(dp.DMA1, ccdr.peripheral.DMA1);

        Self {
            console: ConsoleDriver::new(tx, dma.0),
            led,
        }
    }
}
