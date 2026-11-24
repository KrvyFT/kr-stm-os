mod image;
mod loader;

use crate::{
    drivers::{console::ConsoleDriver, qspi::W25q64jv},
    trap,
};
use image::Image;
pub(crate) use image::ImageError;

pub(crate) const APP_START: u32 = 0x2406_0000;
const APP_END: u32 = 0x2407_f000;
pub(crate) const APP_STACK_TOP: u32 = 0x2408_0000;

pub fn inspect(flash: &mut W25q64jv, console: &mut ConsoleDriver) {
    let result = (|| {
        let mut image = Image::open(flash)?;
        console.print_fmt(format_args!("KAPP: {} app(s)\r\n", image.app_count()));

        for index in 0..image.app_count() {
            let app = image.validate_app(index)?;
            console.print_fmt(format_args!(
                "[{index}] {} entry={:#010x}\r\n",
                app.name(),
                app.entry(),
            ));

            for segment in app.segments() {
                console.print_fmt(format_args!(
                    "  dst={:#010x} src={:#x} file={} mem={} flags={:#x}\r\n",
                    segment.dst_addr,
                    segment.payload_offset,
                    segment.file_len,
                    segment.mem_len,
                    segment.flags,
                ));
            }
        }
        Ok::<(), image::ImageError>(())
    })();

    if let Err(error) = result {
        console.print_fmt(format_args!("KAPP error: {error:?}\r\n"));
    }
}

/// # Safety
/// Call in privileged Thread mode on MSP with SVC available.
/// The image must contain trusted apps that use basic exception frames.
pub(crate) unsafe fn run(
    flash: &mut W25q64jv,
    console: &mut ConsoleDriver,
) -> Result<(), ImageError> {
    let mut image = Image::open(flash)?;
    for index in 0..image.app_count() {
        // SAFETY: The app area is idle until run_app, which returns only
        // after the app has released it and restored the kernel context.
        let code = unsafe {
            let app = loader::load(&mut image, index)?;
            trap::run_app(app.entry, app.stack_top)
        };
        console.wait_for_write();
        console.print_fmt(format_args!("app[{index}] exited: {code}\r\n"));
    }
    console.print_fmt(format_args!("Batch complete\r\n"));
    Ok(())
}
