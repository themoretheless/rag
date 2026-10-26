use crate::model::{User, Other, Wrapper};
use crate::contracts::{Render, Provider, Defaulted};

impl User { pub fn save(&self) {} }
impl Other { pub fn save(&self) {} }
impl<T> Wrapper<T> { pub fn get(&self) -> &T { &self.item } }
impl Render for User { fn render(&self) {} }
impl Provider for User {
    type Item = Other;
    fn provide(&self) -> Self::Item { Other }
}
impl Defaulted for User {}
