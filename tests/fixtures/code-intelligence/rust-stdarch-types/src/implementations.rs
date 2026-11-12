extern crate core as corelib;
use corelib::arch::aarch64::*;
use crate::generic::Value;
impl Value for uint8x16_t { fn value(&self) -> u8 { 7 } }
impl Value for uint16x8_t { fn value(&self) -> u8 { 9 } }
