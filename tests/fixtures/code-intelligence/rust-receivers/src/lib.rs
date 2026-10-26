use std::sync::Arc;

pub mod remote;

#[derive(Default)]
pub struct User;
pub struct Other;
pub struct Service;
pub struct Container {
    pub user: User,
    pub other: Other,
}

impl User {
    pub fn save(&self) {}
}
impl Other {
    pub fn save(&self) {}
}
impl Service {
    pub fn user(&self) -> User { User }
    pub fn other(&self) -> Other { Other }
    pub async fn user_async(&self) -> User { User }
    pub fn typed<T: Default>(&self) -> T { T::default() }
}

pub fn factory() -> User { User }

pub async fn exercise(service: Service, container: Container, users: &[User],
                      others: Vec<Other>, shared: Arc<User>, boxed: Box<Other>) {
    service.user().save();
    service.other().save();
    (service.user_async().await).save();
    service.typed::<User>().save();
    container.user.save();
    container.other.save();
    users[0].save();
    others[0].save();
    shared.save();
    boxed.save();
    let inferred = factory();
    inferred.save();
    remote::factory().save();
}
