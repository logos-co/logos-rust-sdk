/// Add two integers. Exposed as a Logos IPC method via c-ffi codegen.
#[no_mangle]
pub extern "C" fn sdk_test_provider_add(a: i64, b: i64) -> i64 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(sdk_test_provider_add(5, 3), 8);
        assert_eq!(sdk_test_provider_add(-1, 1), 0);
        assert_eq!(sdk_test_provider_add(0, 0), 0);
    }
}
