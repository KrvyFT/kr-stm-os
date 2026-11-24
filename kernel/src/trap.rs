use core::{arch::global_asm, slice};

use crate::{
    batch::{APP_STACK_TOP, APP_START},
    drivers::console::{self, MAX_MESSAGE_BYTES, WriteError},
};

#[repr(C)]
struct BasicExceptionFrame {
    r0: u32,
    r1: u32,
    r2: u32,
    r3: u32,
    r12: u32,
    lr: u32,
    pc: u32,
    xpsr: u32,
}

unsafe extern "C" {
    fn __kr_run_app(frame: *mut BasicExceptionFrame) -> i32;
}

/// # Safety
/// The entry and stack must belong to the loaded, trusted application.
/// Call in privileged Thread mode on MSP, with SVC available and no active app.
/// The kernel and the app must use basic exception frames.
pub(crate) unsafe fn run_app(entry: u32, stack_top: u32) -> i32 {
    debug_assert_eq!(stack_top & 7, 0);
    debug_assert_ne!(entry & 1, 0);

    let frame = (stack_top as usize - size_of::<BasicExceptionFrame>()) as *mut BasicExceptionFrame;

    unsafe {
        frame.write(BasicExceptionFrame {
            r0: 0,
            r1: 0,
            r2: 0,
            r3: 0,
            r12: 0,
            lr: 0,
            pc: entry & !1,
            xpsr: 1 << 24,
        });
        __kr_run_app(frame)
    }
}

#[unsafe(no_mangle)]
extern "C" fn __kr_sys_write(addr: u32, len: u32) -> i32 {
    const EAGAIN: i32 = -11;
    const EFAULT: i32 = -14;
    const EMSGSIZE: i32 = -90;

    if len == 0 {
        return 0;
    }
    if len as usize > MAX_MESSAGE_BYTES {
        return EMSGSIZE;
    }
    if addr < APP_START || addr.checked_add(len).is_none_or(|end| end > APP_STACK_TOP) {
        return EFAULT;
    }

    // SAFETY: The current batch runner admits trusted apps whose sys_write
    // passes an initialized byte slice. The range check covers code/data and PSP.
    let bytes = unsafe { slice::from_raw_parts(addr as *const u8, len as usize) };
    match console::try_write(bytes) {
        Ok(()) => len as i32,
        Err(WriteError::TooLong) => EMSGSIZE,
        Err(WriteError::Full) => EAGAIN,
    }
}

global_asm!(include_str!("trap/svc.s"), options(raw));
