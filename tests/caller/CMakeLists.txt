cmake_minimum_required(VERSION 3.14)
project(SdkTestCallerModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json
               ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(
    NAME sdk_test_caller_module
)

# ── Rust static library ───────────────────────────────────────────────────────
# Contains the logos_module_* C ABI exports (generated scaffold) + the author
# impl + the logos-rust-sdk rlib. Its lp_* undefineds resolve against the
# logos-protocol archive already in the plugin link (via logos-qt-sdk) — ONE
# protocol stack shared by the Qt glue and the Rust SDK, so the host token the
# glue forwards through logos_module_accept_token authenticates this module's
# outbound calls. Re-mention the protocol target after the Rust archive so
# single-pass linkers (GNU ld) see it later on the line.
find_library(LIBSDK_TEST_CALLER
    NAMES libsdk_test_caller.a sdk_test_caller
    PATHS ${CMAKE_CURRENT_SOURCE_DIR}/lib
    NO_DEFAULT_PATH
)

if(NOT LIBSDK_TEST_CALLER)
    message(FATAL_ERROR
        "Rust static library (libsdk_test_caller.a) not found in ${CMAKE_CURRENT_SOURCE_DIR}/lib. "
        "Ensure the preConfigure hook staged the library."
    )
endif()

target_link_libraries(sdk_test_caller_module_module_plugin PRIVATE
    ${LIBSDK_TEST_CALLER}
    logos-protocol::logos_protocol
)

# ── Platform-specific Rust runtime deps ──────────────────────────────────────
if(APPLE)
    target_link_libraries(sdk_test_caller_module_module_plugin PRIVATE
        "-framework CoreFoundation"
        "-framework Security"
    )
else()
    target_link_libraries(sdk_test_caller_module_module_plugin PRIVATE pthread dl)
endif()
