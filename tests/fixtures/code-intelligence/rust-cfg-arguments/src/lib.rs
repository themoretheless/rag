mod left {
    pub fn selected() -> u32 {
        11
    }
}
mod right {
    pub fn selected() -> u32 {
        22
    }
}
#[cfg(feature = "fast")]
use crate::left::selected;
#[cfg(not(feature = "fast"))]
use crate::right::selected;

fn sink(value: u32) -> u32 {
    value
}

pub fn entry() -> u32 {
    sink(
        #[cfg(feature = "fast")]
        selected(),
        #[cfg(not(feature = "fast"))]
        {
            selected()
        },
    )
}

#[test]
fn config_test() {
    let value = entry();
    let expected = selected();
    let branch = body_branch();
    assert_eq!(value, expected);
    assert_eq!(branch, expected);
}

pub fn body_branch() -> u32 {
    #[cfg(feature = "fast")]
    {
        left::selected()
    }
    #[cfg(not(feature = "fast"))]
    {
        right::selected()
    }
}
