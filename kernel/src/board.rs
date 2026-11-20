use stm32h7xx_hal::{
    dma::dma::StreamsTuple,
    gpio::{GpioExt, Output, Speed, gpioc::PC13},
    pac,
    pwr::PwrExt,
    rcc::RccExt,
    serial::SerialExt,
    time::{Hertz, U32Ext},
    xspi::{Qspi, XspiExt},
};

use crate::console::ConsoleDriver;

pub struct Board {
    pub console: ConsoleDriver,
    pub led: PC13<Output>,
    pub qspi: Qspi<pac::QUADSPI>,
}

impl Board {
    pub fn init() -> Self {
        let dp = pac::Peripherals::take().unwrap();

        let power = dp.PWR.constrain().freeze();
        let ccdr = dp.RCC.constrain().freeze(power, &dp.SYSCFG);

        let gpioa = dp.GPIOA.split(ccdr.peripheral.GPIOA);
        let gpiob = dp.GPIOB.split(ccdr.peripheral.GPIOB);
        let gpioc = dp.GPIOC.split(ccdr.peripheral.GPIOC);
        let gpiod = dp.GPIOD.split(ccdr.peripheral.GPIOD);
        let gpioe = dp.GPIOE.split(ccdr.peripheral.GPIOE);

        let led = gpioc.pc13.into_push_pull_output();

        let tx = gpioa.pa9.into_alternate();
        let rx = gpioa.pa10.into_alternate();
        let serial = dp
            .USART1
            .serial((tx, rx), 19_200.bps(), ccdr.peripheral.USART1, &ccdr.clocks)
            .unwrap();

        let (tx, _rx) = serial.split();
        let dma = StreamsTuple::new(dp.DMA1, ccdr.peripheral.DMA1);

        let _ncs = gpiob.pb6.into_alternate::<10>().speed(Speed::VeryHigh);
        let clk = gpiob.pb2.into_alternate::<9>().speed(Speed::VeryHigh);
        let io0 = gpiod.pd11.into_alternate::<9>().speed(Speed::VeryHigh);
        let io1 = gpiod.pd12.into_alternate::<9>().speed(Speed::VeryHigh);
        let io2 = gpioe.pe2.into_alternate::<9>().speed(Speed::VeryHigh);
        let io3 = gpiod.pd13.into_alternate::<9>().speed(Speed::VeryHigh);

        let qspi = dp.QUADSPI.bank1(
            (clk, io0, io1, io2, io3),
            Hertz::from_raw(1_000_000),
            &ccdr.clocks,
            ccdr.peripheral.QSPI,
        );

        Self {
            console: ConsoleDriver::new(tx, dma.0),
            led,
            qspi: qspi,
        }
    }
}
