pub trait Done {
    fn again(self) -> Self;
    fn ready(&self) -> u8;
}
pub trait OtherDone {
    fn again(self) -> Self;
    fn ready(&self) -> u8;
}
pub trait Factory {
    #[cfg(not(feature = "alternate"))]
    type Output: Done;
    #[cfg(feature = "alternate")]
    type Output: OtherDone;
    fn make() -> Self;
    fn output(self) -> Self::Output;
}
pub struct User;
impl Done for User {
    fn again(self) -> Self { self }
    fn ready(&self) -> u8 { 7 }
}
impl OtherDone for User {
    fn again(self) -> Self { self }
    fn ready(&self) -> u8 { 9 }
}
pub struct Producer;
impl Factory for Producer {
    type Output = User;
    fn make() -> Self { Producer }
    fn output(self) -> Self::Output { User }
}
pub fn run<T: Factory>() -> u8 {
    T::make().output().again().ready()
}
#[test]
fn values() {
    #[cfg(not(feature = "alternate"))]
    assert_eq!(run::<Producer>(), 7);
    #[cfg(feature = "alternate")]
    assert_eq!(run::<Producer>(), 9);
}
