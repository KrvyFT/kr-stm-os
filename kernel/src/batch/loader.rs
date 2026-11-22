use core::ptr;

use super::APP_STACK_TOP;
use super::image::{Image, ImageError};

pub struct LoadedApp {
    pub entry: u32,
    pub stack_top: u32,
}

pub unsafe fn load(image: &mut Image<'_>, index: u32) -> Result<LoadedApp, ImageError> {
    let app = image.validate_app(index)?;
    let mut buf = [0u8; 128];
    for segment in app.segments() {
        let dst = segment.dst_addr as *mut u8;
        let mut copied = 0;
        while copied < segment.file_len {
            let len = (segment.file_len - copied).min(buf.len() as u32) as usize;
            image.read_payload(segment.payload_offset, &mut buf)?;
            unsafe {
                ptr::copy_nonoverlapping(buf.as_ptr(), dst.add(copied as usize), len);
            }
            copied += len as u32;
        }
        let zero_len = segment.mem_len - segment.file_len;
        if zero_len != 0 {
            unsafe {
                ptr::write_bytes(dst.add(segment.file_len as usize), 0, zero_len as usize);
            }
        }
    }

    Ok(LoadedApp {
        entry: app.entry(),
        stack_top: APP_STACK_TOP,
    })
}
