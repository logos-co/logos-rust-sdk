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

    /// Build a deterministic blob of `size` bytes and emit it on the
    /// `blobReady` event, returning the byte count. The blob deliberately
    /// contains 0x00 and bytes >= 0x80 — values a text encoding would mangle.
    /// The event parameter is named `payload` on purpose: it is the exact shape
    /// of delivery_module's messageReceived(..., payload: bstr, ...), and it
    /// makes the generated `emit_blob_ready` a regression guard for the emitter
    /// accumulator-shadowing bug (a `payload` arg colliding with the local args
    /// vector would not compile).
    fn emit_blob(&mut self, size: i64) -> i64 {
        let n = size.max(0) as usize;
        let mut payload = Vec::with_capacity(n);
        for i in 0..n {
            payload.push(((i as u64).wrapping_mul(7).wrapping_add(11) & 0xff) as u8);
        }
        emit_blob_ready(0, &payload);
        payload.len() as i64
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
