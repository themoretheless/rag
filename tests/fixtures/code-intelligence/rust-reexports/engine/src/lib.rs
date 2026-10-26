mod internal;

mod stage {
    #[cfg(feature = "fast")]
    pub use crate::internal::fast_work as bridge;
    #[cfg(not(feature = "fast"))]
    pub use crate::internal::slow_work as bridge;
}

pub mod api {
    pub use crate::internal::Worker as Task;
    pub(crate) use crate::internal::restricted;
    pub use crate::stage::bridge as run;
}

pub mod recursive {
    pub use crate::ring::*;
}
pub mod ring {
    pub use crate::internal::globbed;
    pub use crate::recursive::*;
}

pub fn internal_entry() {
    api::restricted();
}

#[test]
fn selected_export() {
    let actual = api::run();
    #[cfg(feature = "fast")]
    assert_eq!(actual, 11);
    #[cfg(not(feature = "fast"))]
    assert_eq!(actual, 22);
}
