pub trait Mark { fn mark(self) -> u8; fn again(self) -> Self; }
impl<T> Mark for *const T { fn mark(self) -> u8 { 11 } fn again(self) -> Self { self } }
impl<T> Mark for *mut T { fn mark(self) -> u8 { 22 } fn again(self) -> Self { self } }
pub fn inspect(c: *const u8, m: *mut u8) -> [u8; 4] {
    [c.mark(), m.mark(), (m as *const u8).mark(), c.again().mark()]
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn pointer_owner_identity() {
        let mut byte=1u8;
        let m=&mut byte as *mut u8;
        assert_eq!(inspect(m as *const u8, m), [11,22,11,11]);
    }
}
