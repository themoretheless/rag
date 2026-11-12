use core::ops::FnOnce as Callback;
use core::option::Option as Maybe;
pub struct User;
impl User { pub fn save(&self) -> u8 { 7 } }
pub struct Other;
impl Other { pub fn save(&self) -> u8 { 9 } }
pub fn optional(mut f: impl FnMut() -> Option<User>) -> Option<u8> {
    let value = f()?;
    Some(value.save())
}
pub fn pointer(f: fn(u8) -> Result<Other, ()>) -> Result<u8, ()> {
    Ok(f(1)?.save())
}
pub fn generic<F>(f: F) -> Maybe<u8> where F: Callback() -> Maybe<User> {
    Some(f()?.save())
}
pub fn borrowed(f: &mut dyn FnMut() -> Option<Other>) -> Option<u8> {
    Some(f()?.save())
}
pub fn factory() -> Result<User, ()> { Ok(User) }
pub fn direct() -> Result<u8, ()> { Ok(factory()?.save()) }
#[test]
fn values() {
    assert_eq!(optional(|| Some(User)), Some(7));
    assert_eq!(pointer(|_| Ok(Other)), Ok(9));
    assert_eq!(generic(|| Some(User)), Some(7));
    assert_eq!(borrowed(&mut || Some(Other)), Some(9));
    assert_eq!(direct(), Ok(7));
    assert_eq!(optional(|| None), None);
    assert_eq!(pointer(|_| Err(())), Err(()));
}
