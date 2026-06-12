//! Pure-Rust Logos module on the common cdylib authoring path.
//!
//! The module-impl C ABI exports (logos_module_*) come from the scaffold
//! lidl-gen generates from the .lidl contract at build time; the author
//! code is just this trait impl plus the install hook. No unsafe, no
//! hand-written C signatures, no manual string memory management.

include!(concat!(env!("OUT_DIR"), "/provider_gen.rs"));

#[derive(Default)]
struct RustProviderImpl;

impl RustProviderModule for RustProviderImpl {
    /// Add two integers.
    fn add(&mut self, a: i64, b: i64) -> i64 {
        a + b
    }

    /// Multiply two integers (saturating on overflow).
    fn multiply(&mut self, a: i64, b: i64) -> i64 {
        a.saturating_mul(b)
    }

    /// Compute n! (factorial). Returns -1 on overflow or negative input.
    fn factorial(&mut self, n: i64) -> i64 {
        if n < 0 {
            return -1;
        }
        let mut result: i64 = 1;
        for i in 2..=n {
            result = match result.checked_mul(i) {
                Some(v) => v,
                None => return -1,
            };
        }
        result
    }

    /// Compute the nth Fibonacci number. Returns -1 on overflow or negative input.
    fn fibonacci(&mut self, n: i64) -> i64 {
        if n < 0 {
            return -1;
        }
        if n < 2 {
            return n;
        }
        let (mut a, mut b) = (0i64, 1i64);
        for _ in 2..=n {
            let next = match a.checked_add(b) {
                Some(v) => v,
                None => return -1,
            };
            a = b;
            b = next;
        }
        b
    }

    /// Return 1 if n is prime, 0 otherwise.
    fn is_prime(&mut self, n: i64) -> i64 {
        if n < 2 {
            return 0;
        }
        if n == 2 {
            return 1;
        }
        if n % 2 == 0 {
            return 0;
        }
        let mut i = 3i64;
        while i * i <= n {
            if n % i == 0 {
                return 0;
            }
            i += 2;
        }
        1
    }

    /// Greet the given name — strings cross the boundary as plain `String`s.
    fn greet(&mut self, name: String) -> String {
        let who = if name.is_empty() { "World".to_string() } else { name };
        format!("Hello, {}! (from Rust provider)", who)
    }

    /// Return the provider library version string.
    fn lib_version(&mut self) -> String {
        "1.0.0".to_string()
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<RustProviderImpl>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(RustProviderImpl.add(2, 3), 5);
    }

    #[test]
    fn test_factorial() {
        assert_eq!(RustProviderImpl.factorial(5), 120);
    }

    #[test]
    fn test_is_prime() {
        assert_eq!(RustProviderImpl.is_prime(7), 1);
        assert_eq!(RustProviderImpl.is_prime(4), 0);
    }
}
