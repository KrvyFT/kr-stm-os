use core::{
    arch::global_asm,
    ptr, slice,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use cortex_m::{
    asm,
    peripheral::{MPU, SCB},
    register::psp,
};

use crate::{
    batch::{APP_END, APP_STACK_TOP, APP_START},
    drivers::console::{self, MAX_MESSAGE_BYTES, WriteError},
};

const THREAD_MSP_BASIC: u32 = 0xffff_fff9;
const THREAD_PSP_BASIC: u32 = 0xffff_fffd;
const SVC_EXCEPTION: u32 = 11;
const SVC_EXIT: u16 = 0xdf01;
const SVC_WRITE: u16 = 0xdf02;
// The assembly save area holds r3-r11 and EXC_RETURN.
const KERNEL_SAVE_BYTES: usize = 10 * size_of::<u32>();

static APP_ACTIVE: AtomicBool = AtomicBool::new(false);
static FAULT_PC: AtomicU32 = AtomicU32::new(0);
static FAULT_CFSR: AtomicU32 = AtomicU32::new(0);

pub(crate) fn init_app_faults() {
    const MEMFAULTENA: u32 = 1 << 16;
    const USGFAULTENA: u32 = 1 << 18;

    unsafe {
        (*SCB::PTR)
            .shcsr
            .modify(|value| value | MEMFAULTENA | USGFAULTENA);
    }
    init_app_mpu();
}

fn init_app_mpu() {
    const REGION_SIZE: u32 = APP_STACK_TOP - APP_START;
    const _: () = assert!(REGION_SIZE.is_power_of_two() && APP_START % REGION_SIZE == 0);
    const RASR_SIZE: u32 = (REGION_SIZE.ilog2() - 1) << 1;
    const RASR_ENABLE: u32 = 1;
    const RASR_FULL_ACCESS: u32 = 0b011 << 24;
    const RASR_NORMAL_WB_WA: u32 = (1 << 19) | (1 << 17) | (1 << 16);
    const MPU_ENABLE: u32 = 1;
    const MPU_PRIVDEFENA: u32 = 1 << 2;

    // The app has one region for its code, data, and PSP stack. The kernel
    // uses the default privileged map for flash, its own RAM, and devices.
    let mpu = unsafe { &*MPU::PTR };
    let regions = (mpu._type.read() >> 8) & 0xff;
    assert!(regions != 0, "application isolation requires an MPU");

    asm::dsb();
    unsafe {
        mpu.ctrl.write(0);
        // An inherited region must not grant the app access outside its slot.
        for region in 0..regions {
            mpu.rnr.write(region);
            mpu.rasr.write(0);
        }
        mpu.rnr.write(0);
        mpu.rbar.write(APP_START);
        mpu.rasr
            .write(RASR_FULL_ACCESS | RASR_NORMAL_WB_WA | RASR_SIZE | RASR_ENABLE);
        mpu.ctrl.write(MPU_ENABLE | MPU_PRIVDEFENA);
    }
    asm::dsb();
    asm::isb();
}

// The caller must validate the basic PSP frame before invoking this handler.
unsafe fn handle_app_fault(frame: *const BasicExceptionFrame, exception: u32) -> bool {
    const UNDEFINSTR: u32 = 1 << 16;
    const DACCVIOL: u32 = 1 << 1;
    const MMARVALID: u32 = 1 << 7;

    let scb = unsafe { &*SCB::PTR };
    let cfsr = scb.cfsr.read();
    let recoverable = match exception {
        6 => cfsr == UNDEFINSTR,
        4 => (cfsr & !MMARVALID) == DACCVIOL,
        _ => false,
    };
    if !recoverable {
        return false;
    }

    let pc = unsafe { ptr::addr_of!((*frame).pc).read_volatile() };
    FAULT_PC.store(pc, Ordering::Relaxed);
    FAULT_CFSR.store(cfsr, Ordering::Release);

    // CFSR is write-one-to-clear. Do not leave these flags for the next app.
    unsafe { scb.cfsr.write(cfsr) };
    asm::dsb();
    true
}

pub(crate) fn take_app_fault() -> Option<(u32, u32)> {
    let cfsr = FAULT_CFSR.swap(0, Ordering::AcqRel);
    (cfsr != 0).then(|| (FAULT_PC.load(Ordering::Relaxed), cfsr))
}

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

#[repr(u32)]
enum ExceptionAction {
    ResumeApp,
    StartApp,
    FinishApp,
    DefaultHandler,
}

unsafe extern "C" {
    fn __kr_run_app(frame: *mut BasicExceptionFrame) -> i32;
    static __kr_resume_kernel: u8;
    fn DefaultHandler();
}

global_asm!(
    include_str!("trap/svc.s"),
    dispatch = sym dispatch_exception,
    resume = const ExceptionAction::ResumeApp as u32,
    start = const ExceptionAction::StartApp as u32,
    finish = const ExceptionAction::FinishApp as u32,
    psp_basic_inverted = const !THREAD_PSP_BASIC,
    default_handler = sym DefaultHandler,
);

fn app_frame(stack_floor: u32) -> Option<*mut BasicExceptionFrame> {
    let address = psp::read();
    let last_frame_start = APP_STACK_TOP - size_of::<BasicExceptionFrame>() as u32;
    (address & 7 == 0 && (stack_floor..=last_frame_start).contains(&address))
        .then_some(address as *mut BasicExceptionFrame)
}

// Called only from the exception entry with MSP captured before its temporary push.
unsafe extern "C" fn dispatch_exception(
    msp: *mut u8,
    exc_return: u32,
    exception: u32,
) -> ExceptionAction {
    let app_active = APP_ACTIVE.load(Ordering::Acquire);
    if exception == SVC_EXCEPTION {
        if app_active && exc_return == THREAD_MSP_BASIC {
            let frame = msp.cast::<BasicExceptionFrame>();
            let pc = unsafe { ptr::addr_of!((*frame).pc).read_volatile() };
            if pc == (&raw const __kr_resume_kernel) as u32 {
                let initial_psp = unsafe { ptr::addr_of!((*frame).r0).read_volatile() };
                // Handler mode is still using MSP, so PSP can be prepared in Rust.
                unsafe { psp::write(initial_psp) };
                return ExceptionAction::StartApp;
            }
        }

        if app_active
            && exc_return == THREAD_PSP_BASIC
            && let Some(frame) = app_frame(APP_START)
        {
            return unsafe { dispatch_app_svc(msp, frame) };
        }
        asm::udf();
    }

    if app_active
        && exc_return == THREAD_PSP_BASIC
        && let Some(frame) = app_frame(APP_END)
        && unsafe { handle_app_fault(frame, exception) }
    {
        return unsafe { finish_app(msp, 0) };
    }
    ExceptionAction::DefaultHandler
}

unsafe fn dispatch_app_svc(msp: *mut u8, frame: *mut BasicExceptionFrame) -> ExceptionAction {
    // The hardware-stacked PC points after the 16-bit Thumb SVC instruction.
    let pc = unsafe { ptr::addr_of!((*frame).pc).read_volatile() };
    if pc & 1 != 0 || !(APP_START + 2..=APP_STACK_TOP).contains(&pc) {
        asm::udf();
    }
    let instruction = unsafe { ((pc - 2) as *const u16).read_volatile() };
    let arg0 = unsafe { ptr::addr_of!((*frame).r0).read_volatile() };
    match instruction {
        SVC_EXIT => unsafe { finish_app(msp, arg0) },
        SVC_WRITE => {
            let len = unsafe { ptr::addr_of!((*frame).r1).read_volatile() };
            let result = sys_write(arg0, len);
            unsafe { ptr::addr_of_mut!((*frame).r0).write_volatile(result as u32) };
            ExceptionAction::ResumeApp
        }
        _ => asm::udf(),
    }
}

unsafe fn finish_app(msp: *mut u8, code: u32) -> ExceptionAction {
    // The suspended kernel's hardware frame follows the entry's software save area.
    // Write r0 there so hardware unstacking delivers the code to __kr_run_app.
    let kernel_frame = msp
        .wrapping_add(KERNEL_SAVE_BYTES)
        .cast::<BasicExceptionFrame>();
    unsafe { ptr::addr_of_mut!((*kernel_frame).r0).write_volatile(code) };
    ExceptionAction::FinishApp
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
        APP_ACTIVE.store(true, Ordering::Release);
        let code = __kr_run_app(frame);
        APP_ACTIVE.store(false, Ordering::Release);
        code
    }
}

fn sys_write(addr: u32, len: u32) -> i32 {
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
