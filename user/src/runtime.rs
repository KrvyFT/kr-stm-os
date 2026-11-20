use core::panic::PanicInfo;

use crate::{exit, println};

const PANIC_EXIT_CODE: i32 = 1;

unsafe extern "Rust" {
    fn main() -> i32;
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let code = unsafe { main() };
    exit(code)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        println!(
            "Panicked at {}:{} {}",
            location.file(),
            location.line(),
            info.message()
        );
    } else {
        println!("Panicked: {}", info.message());
    }
    exit(PANIC_EXIT_CODE)
}
