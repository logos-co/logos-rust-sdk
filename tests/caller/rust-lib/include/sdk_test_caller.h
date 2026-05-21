#ifndef SDK_TEST_CALLER_H
#define SDK_TEST_CALLER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Call sdk_test_provider_module.add(a, b) via IPC.
 * Returns the sum, or -1 on error.
 */
int64_t sdk_test_caller_call_add(int64_t a, int64_t b);

#ifdef __cplusplus
}
#endif

#endif /* SDK_TEST_CALLER_H */
