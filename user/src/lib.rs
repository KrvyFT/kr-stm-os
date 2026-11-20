#![no_std]

mod console;
mod runtime;
mod syscall;

// Exported macros need a crate-root path that external apps can access.
#[doc(hidden)]
pub use console::_print;

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
