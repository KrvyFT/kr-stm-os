use core::{hint, panic::PanicInfo};

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    if let Some(location) = info.location() {
        todo!()
    } else {
        todo!()
    }

    todo!()
}
