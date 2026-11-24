mod image;
mod loader;

use core::fmt::{self, Write};

use crate::{
    console::{self, ConsoleDriver},
    qspi::W25q64jv,
};
use image::Image;

const APP_START: u32 = 0x2406_0000;
const APP_END: u32 = 0x2407_f000;
const APP_STACK_TOP: u32 = 0x2408_0000;

fn print_line(console: &mut ConsoleDriver, args: fmt::Arguments<'_>) {
    let mut line = heapless::String::<128>::new();
    line.write_fmt(args).unwrap();
    console::try_print(line.as_str()).unwrap();
    console.wait_for_write();
}

pub fn inspect(flash: &mut W25q64jv, console: &mut ConsoleDriver) {
    let result = (|| {
        let mut image = Image::open(flash)?;
        print_line(
            console,
            format_args!("KAPP: {} app(s)\r\n", image.app_count()),
        );

        for index in 0..image.app_count() {
            let app = image.validate_app(index)?;
            print_line(
                console,
                format_args!("[{index}] {} entry={:#010x}\r\n", app.name(), app.entry(),),
            );

            for segment in app.segments() {
                print_line(
                    console,
                    format_args!(
                        "  dst={:#010x} src={:#x} file={} mem={} flags={:#x}\r\n",
                        segment.dst_addr,
                        segment.payload_offset,
                        segment.file_len,
                        segment.mem_len,
                        segment.flags,
                    ),
                );
            }
        }
        Ok::<(), image::ImageError>(())
    })();

    if let Err(error) = result {
        print_line(console, format_args!("KAPP error: {error:?}\r\n"));
    }
}
