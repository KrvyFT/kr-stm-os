#![no_std]
#![no_main]

mod batch;
mod board;
mod drivers;
mod panic_handler;
mod trap;

use cortex_m_rt::entry;

use crate::{board::Board, drivers::console};

#[entry]
fn main() -> ! {
    let mut board = Board::init();

    let _ = console::try_print("\r\nkr-stm-os booted\r\n");
    board.led.set_low();
    board.console.wait_for_write();
    board.led.set_high();

    // SAFETY: Boot is in privileged Thread mode on MSP. The batch image
    // is provisioned with trusted apps that use the basic exception frame.
    let result = unsafe { batch::run(&mut board.flash, &mut board.console) };
    if let Err(error) = result {
        board
            .console
            .print_fmt(format_args!("KAPP error: {error:?}\r\n"));
    }

    panic!("Done!");
}
