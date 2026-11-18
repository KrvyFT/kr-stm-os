#![no_std]
#![no_main]

mod board;
mod console;
mod panic_handler;

use cortex_m_rt::entry;

use board::Board;

#[entry]
fn main() -> ! {
    let mut board = Board::init();

    let _ = console::try_print("kr-stm-os booted\r\n");
    board.led.set_low();
    board.console.wait_for_write();
    board.led.set_high();

    panic!("Done!");
}
