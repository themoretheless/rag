pub trait Value { fn value(&self) -> u8; }
pub struct Generic<T>(pub T);
impl<V: Value> Generic<V> {
    pub fn get(&self) -> u8 { self.0.value() }
}
pub struct Decoy;
impl Decoy { pub fn get(&self) -> u8 { 99 } }
