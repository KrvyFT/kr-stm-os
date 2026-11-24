use core::arch::asm;

const SYS_EXIT: u8 = 1;
const SYS_WRITE: u8 = 2;

fn syscall<const NUMBER: u8>(arg0: usize, arg1: usize) -> i32 {
    let result: i32;
    unsafe {
        asm!(
            "svc {number}",
            number = const NUMBER,
            inlateout("r0") arg0 => result,
            in("r1") arg1,
        );
    }

    result
}

pub(super) fn sys_write(bytes: &[u8]) -> i32 {
    syscall::<SYS_WRITE>(bytes.as_ptr() as usize, bytes.len())
}

pub(super) fn sys_exit(code: i32) -> i32 {
    syscall::<SYS_EXIT>(code as usize, 0)
}
