pub fn fast_work() -> u32 {
    11
}
pub fn slow_work() -> u32 {
    22
}
pub fn globbed() {}
pub(crate) fn restricted() {}

pub struct Worker;
impl Worker {
    pub fn make() -> u32 {
        7
    }
}
