use core::{fmt::Write, hint::spin_loop, panic::PanicInfo};

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    cortex_m::interrupt::disable();

    if let Some(mut console) = crate::console::get_console() {
        let _ = write!(console, "\r\n[kernel panic] {info}\r\n");
    }

    loop {
        spin_loop();
    }
}
