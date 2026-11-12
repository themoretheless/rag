#[derive(Clone, Copy)] pub struct Fast;
#[derive(Clone, Copy)] pub struct Slow;
impl Fast { pub fn value(&self)->u8 {11} }
impl Slow { pub fn value(&self)->u8 {22} }
pub union Holder {
    #[cfg(feature="fast")] value:Fast,
    #[cfg(not(feature="fast"))] value:Slow,
}
impl Holder { pub unsafe fn run(&self)->u8 { self.value.value() } }
pub union Slot<T:Copy> { pub item:T, pub spare:u32 }
#[allow(non_snake_case)] pub fn Slot()->Fast{Fast}
pub struct Envelope { pub slot:Slot<Slow> }
pub unsafe fn generic(g:Slot<Fast>)->u8 { g.item.value() }
pub unsafe fn nested(h:Envelope)->u8 { h.slot.item.value() }
pub unsafe fn again(h:&Holder)->u8 { h.run() }
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn initialized_union_owner_identity() {
        #[cfg(feature="fast")] let holder=Holder{value:Fast};
        #[cfg(not(feature="fast"))] let holder=Holder{value:Slow};
        #[cfg(feature="fast")] let expected=11;
        #[cfg(not(feature="fast"))] let expected=22;
        unsafe {
            assert_eq!(holder.run(),expected);
            assert_eq!(generic(Slot{item:Fast}),11);
            assert_eq!(nested(Envelope{slot:Slot{item:Slow}}),22);
            assert_eq!(again(&holder),expected);
            assert_eq!(Slot().value(),11);
        }
    }
}
