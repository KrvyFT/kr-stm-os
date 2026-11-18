use core::{hint::spin_loop, panic::PanicInfo};

use crate::console;

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    if let Some(location) = info.location() {
        console::write_panic(format_args!(
            "\r\n\x1b[31m[kernel panic]\r\nPanicked at {}:{} {}\x1b[0m\r\n",
            location.file(),
            location.line(),
            info.message()
        ));
    } else {
        console::write_panic(format_args!(
            "\r\n\x1b[31m[kernel panic]\r\nPanicked: {}\x1b[0m\r\n",
            info.message()
        ));
    }

    loop {
        spin_loop();
    }
}
