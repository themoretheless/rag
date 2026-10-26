pub trait Render { fn render(&self); }
pub trait Provider { type Item; fn provide(&self) -> Self::Item; }
pub trait Defaulted { fn default_action(&self) {} }
