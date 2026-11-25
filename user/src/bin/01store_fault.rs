#![no_std]
#![no_main]

use user_lib::println;

#[unsafe(no_mangle)]
fn main() -> i32 {
    // This address is outside the app's MPU region and past the ITCM range.
    const FORBIDDEN_ADDRESS: u32 = 0x0001_0000;
    println!("Testing an MPU-protected write to {FORBIDDEN_ADDRESS:#010x}");
    unsafe {
        core::arch::asm!(
            "strb {value}, [{address}]",
            address = in(reg) FORBIDDEN_ADDRESS,
            value = in(reg) 0u32,
            options(nostack, preserves_flags),
        );
    }
    // A normal exit here means the store unexpectedly succeeded.
    57
}
