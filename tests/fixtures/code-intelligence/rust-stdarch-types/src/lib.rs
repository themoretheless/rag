pub mod generic;
mod implementations;
use core::arch::aarch64::uint8x16_t as ByteVector;
use std::arch::aarch64::uint16x8_t as WordVector;
pub struct Bytes(pub generic::Generic<ByteVector>);
impl Bytes { pub fn value(&self) -> u8 { self.0.get() } }
pub struct Words(pub generic::Generic<WordVector>);
impl Words { pub fn value(&self) -> u8 { self.0.get() } }
pub fn selected(value: ByteVector) -> u8 { <ByteVector as generic::Value>::value(&value) }
#[test]
fn values() {
    // These unsigned integer SIMD structs admit an all-zero value.
    let bytes = Bytes(generic::Generic(unsafe { core::mem::zeroed() }));
    let words = Words(generic::Generic(unsafe { core::mem::zeroed() }));
    assert_eq!((bytes.value(), words.value(), selected(bytes.0.0)), (7, 9, 7));
}
