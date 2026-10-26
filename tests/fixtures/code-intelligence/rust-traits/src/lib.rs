pub mod model;
pub mod contracts;
pub mod implementations;

use model::{User, Wrapper};
use contracts::{Render as VisibleRender, Provider, Defaulted};

pub fn concrete(user: User, wrapped: Wrapper<User>) {
    user.save();
    user.render();
    user.provide().save();
    wrapped.get().save();
    <User as VisibleRender>::render(&user);
    user.default_action();
}
pub fn bounded<T: VisibleRender>(value: T) { value.render(); }
pub fn dynamic(value: &dyn VisibleRender) { value.render(); }
pub fn selected<T: Provider>() -> T::Item { panic!("fixture is never executed") }
pub fn projection(user: User) {
    selected::<User>().save();
    let item: <User as Provider>::Item = user.provide();
    item.save();
}
