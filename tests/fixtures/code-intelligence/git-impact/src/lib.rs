pub mod worker;
pub fn api() -> i32 { worker::value() }
pub fn entry() -> i32 { api() }
