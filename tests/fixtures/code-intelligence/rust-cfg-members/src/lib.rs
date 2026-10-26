#[cfg(any())]
compile_error!("the disabled configuration must not expand this item");

pub struct Fast;
impl Fast {
    pub fn value(&self) -> u32 {
        11
    }
}
pub struct Slow;
impl Slow {
    pub fn value(&self) -> u32 {
        22
    }
}

pub struct Holder {
    #[cfg(feature = "fast")]
    item: Fast,
    #[cfg(not(feature = "fast"))]
    item: Slow,
}
impl Holder {
    pub fn run(&self) -> u32 {
        self.item.value()
    }
}

fn fast() -> Fast {
    Fast
}
fn slow() -> Slow {
    Slow
}
pub fn make() -> Holder {
    Holder {
        #[cfg(feature = "fast")]
        item: fast(),
        #[cfg(not(feature = "fast"))]
        item: slow(),
    }
}

pub fn standard(values: standard_library::vec::Vec<Fast>) -> u32 {
    values[0].value()
}
#[cfg(feature = "fast")]
extern crate std as standard_library;
#[cfg(not(feature = "fast"))]
extern crate std as standard_library;

fn live() {}
pub fn call_live() {
    live();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unused(#[cfg(feature = "fast")] _value: u8) {}
    #[test]
    fn field_identity() {
        let value = make().run();
        #[cfg(feature = "fast")]
        assert_eq!(value, 11);
        #[cfg(not(feature = "fast"))]
        assert_eq!(value, 22);
        assert_eq!(standard(standard_library::vec![Fast]), 11);
        call_live();
    }
}
