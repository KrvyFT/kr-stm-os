#![no_std]

mod console;
mod runtime;

// Exported macros need a crate-root path that external apps can access.
#[doc(hidden)]
pub use console::_print;

pub use runtime::{exit, write};
