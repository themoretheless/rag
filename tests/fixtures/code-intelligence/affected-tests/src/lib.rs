pub mod worker;
pub fn api() -> i32 { worker::value() }
#[cfg(not(test))]
pub fn production_only() {}
#[cfg(test)]
mod external_tests;
#[cfg(test)]
mod tests {
    use super::api as imported;
    #[test]
    fn unit_alias() { let result = imported(); assert_eq!(result, 1); }
    #[test] // Comments between attributes and the function are legal.
    fn unit_direct() { let result = super::api(); assert_eq!(result, 1); }
    fn consume(callback: fn() -> i32) { let result = callback(); assert_eq!(result, 1); }
    #[test]
    fn callback_value() { consume(super::api); }
    #[test]
    #[ignore = "explicit opt-in fixture"]
    fn ignored_case() { let result = super::api(); assert_eq!(result, 1); }
    #[doc = "This is not a [test] or [tokio::test attribute."]
    fn misleading_doc() { super::api(); }
    mod nested {
        #[test]
        fn nested_case() { let result = super::super::api(); assert_eq!(result, 1); }
    }
}
