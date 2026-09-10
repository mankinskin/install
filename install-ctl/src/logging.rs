//! Tagged stdout/stderr logging macros (ported from viewer-ctl).
//!
//! No ANSI escape codes: consumers may parse raw stdout text.

#[macro_export]
macro_rules! info {
    ($tag:expr, $($arg:tt)*) => { println!("[{}] {}", $tag, format!($($arg)*)); };
}

#[macro_export]
macro_rules! warn {
    ($tag:expr, $($arg:tt)*) => { println!("[{}] WARN {}", $tag, format!($($arg)*)); };
}

#[macro_export]
macro_rules! error {
    ($tag:expr, $($arg:tt)*) => { eprintln!("[{}] ERROR {}", $tag, format!($($arg)*)); };
}
