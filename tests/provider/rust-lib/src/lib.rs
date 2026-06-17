//! Minimal provider module — logos-rust-sdk integration test fixture, on the
//! common cdylib authoring path: the module-impl C ABI exports come from the
//! scaffold lidl-gen generates from the .lidl contract at build time; the
//! author code is just the trait impl plus the install hook.

include!("provider_gen.rs");

#[derive(Default)]
struct ProviderImpl;

impl SdkTestProviderModule for ProviderImpl {
    fn add(&mut self, a: i64, b: i64) -> i64 {
        a + b
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<ProviderImpl>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        let mut imp = ProviderImpl;
        assert_eq!(imp.add(5, 3), 8);
        assert_eq!(imp.add(-1, 1), 0);
        assert_eq!(imp.add(0, 0), 0);
    }
}
