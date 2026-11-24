mod syscall;

use core::panic::PanicInfo;

use crate::println;

const PANIC_EXIT_CODE: i32 = 1;

pub fn write(bytes: &[u8]) -> i32 {
    syscall::sys_write(bytes)
}

/// Ends the current application with the given status.
pub fn exit(code: i32) -> ! {
    let _ = syscall::sys_exit(code);

    // The kernel should never resume an exited app. Avoid panicking here
    // because the panic handler itself calls exit.
    loop {
        core::hint::spin_loop();
    }
}

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
