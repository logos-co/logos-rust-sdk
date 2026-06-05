#ifndef RUST_PROVIDER_H
#define RUST_PROVIDER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Add two integers. */
int64_t rust_provider_add(int64_t a, int64_t b);

/** Multiply two integers (saturating on overflow). */
int64_t rust_provider_multiply(int64_t a, int64_t b);

/** Compute n! (factorial). Returns -1 on overflow or negative input. */
int64_t rust_provider_factorial(int64_t n);

/** Compute the nth Fibonacci number. Returns -1 on overflow or negative input. */
int64_t rust_provider_fibonacci(int64_t n);

/** Return 1 if n is prime, 0 otherwise. */
int64_t rust_provider_is_prime(int64_t n);

/**
 * Greet the given name.
 * Returns a heap-allocated C string that must be freed with rust_provider_free_string().
 * Passing NULL uses "World" as the name.
 */
char* rust_provider_greet(const char* name);

/** Free a string returned by rust_provider_greet(). */
void rust_provider_free_string(char* ptr);

/** Return the Rust library version string (static, do not free). */
const char* rust_provider_version(void);

#ifdef __cplusplus
}
#endif

#endif /* RUST_PROVIDER_H */
