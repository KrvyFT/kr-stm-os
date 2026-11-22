#![no_std]
#![no_main]

mod batch;
mod board;
mod console;
mod panic_handler;
mod qspi;

use core::fmt::Write;

use cortex_m_rt::entry;

use board::Board;

use crate::qspi::W25q64jv;

#[entry]
fn main() -> ! {
    let mut board = Board::init();

    let _ = console::try_print("\r\nkr-stm-os booted\r\n");
    board.led.set_low();
    board.console.wait_for_write();
    board.led.set_high();

    let mut flash = W25q64jv::new(board.qspi);

    batch::inspect(&mut flash, &mut board.console);

    panic!("Done!");
}
