pub mod worker;
pub fn start() { left(); right(); }
fn left() { worker::bridge(); }
fn right() { worker::bridge(); }
pub fn finish() {}
fn value() { let _callback = finish; }
mod a { pub fn duplicate() {} }
mod b { pub fn duplicate() {} }
