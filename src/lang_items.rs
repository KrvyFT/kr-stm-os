use core::{hint::spin_loop, panic::PanicInfo};

use crate::console;

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    console::write_panic(format_args!("\r\n[kernel panic] {info}\r\n"));

    if let Some(location) = info.location() {
        console::write_panic(format_args!(
            "Panicked at {}:{} {}",
            location.file(),
            location.line(),
            info.message()
        ));
    } else {
        console::write_panic(format_args!("Panicked: {}", info.message()));
    }

    loop {
        spin_loop();
    }
}
