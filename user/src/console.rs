use core::fmt::{self, Write as _};

pub fn _print(args: fmt::Arguments<'_>, newline: bool) -> fmt::Result {
    let mut text = heapless::String::<128>::new();
    text.write_fmt(args)?;

    if newline {
        text.push_str("\r\n").map_err(|_| fmt::Error)?;
    }

    if crate::write(text.as_bytes()) == text.len() as i32 {
        Ok(())
    } else {
        Err(fmt::Error)
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        let _ = $crate::_print(core::format_args!($($arg)*), false);
    }};
}

#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {{
        let _ = $crate::_print(core::format_args!($($arg)*), true);
    }};
}
