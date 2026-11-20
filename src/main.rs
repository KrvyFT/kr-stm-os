#![no_std]
#![no_main]

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

    let id = W25q64jv::new(board.qspi).jedec_id().unwrap();
    let mut text = heapless::String::<16>::new();
    write!(&mut text, "{:02X} {:02X} {:02X}\r\n", id[0], id[1], id[2]).unwrap();

    console::try_print(&text).unwrap();
    board.console.wait_for_write();
    panic!("Done!");
}
