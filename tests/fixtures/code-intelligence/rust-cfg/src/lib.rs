#[cfg(feature = "fast")]
pub fn fast() {}

#[cfg(not(feature = "fast"))]
pub fn slow() {}

#[cfg(feature = "leaf")]
pub mod enabled;

#[cfg(feature = "fast")]
pub fn run() {
    fast();
    enabled::leaf();
}

#[cfg(not(feature = "fast"))]
pub fn run() {
    slow();
}
