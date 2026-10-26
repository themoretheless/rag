pub struct User;
pub struct Other;
impl User { pub fn value(&self) -> u8 { 11 } }
impl Other { pub fn value(&self) -> u8 { 22 } }
pub struct Pair<T>(pub T, pub Other);
pub struct Outer(pub Pair<User>);
pub fn inspect(pair: &Pair<User>, outer: &Outer) -> [u8; 4] {
    [pair.0.value(), pair.1.value(), outer.0.0.value(), outer.0.1.value()]
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn tuple_owner_identity() {
        assert_eq!(inspect(&Pair(User, Other), &Outer(Pair(User, Other))), [11, 22, 11, 22]);
    }
}
