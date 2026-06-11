cmake_minimum_required(VERSION 3.14)
project(SdkTestProviderModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json
               ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(
    NAME sdk_test_provider_module
)

find_library(LIBSDK_TEST_PROVIDER
    NAMES libsdk_test_provider.a sdk_test_provider
    PATHS ${CMAKE_CURRENT_SOURCE_DIR}/lib
    NO_DEFAULT_PATH
)

if(NOT LIBSDK_TEST_PROVIDER)
    message(FATAL_ERROR
        "Rust static library (libsdk_test_provider.a) not found in ${CMAKE_CURRENT_SOURCE_DIR}/lib. "
        "Ensure the preConfigure hook copied the buildRustPackage output."
    )
endif()

# The Rust staticlib provides the logos_module_* exports the generated glue
# calls; its own lp_* undefineds resolve against the logos-protocol archive
# already in the plugin link (via logos-qt-sdk). Re-mention the protocol
# target after the Rust archive so single-pass linkers (GNU ld) see it
# later on the line — one protocol stack shared by glue and Rust.
target_link_libraries(sdk_test_provider_module_module_plugin PRIVATE
    ${LIBSDK_TEST_PROVIDER}
    logos-protocol::logos_protocol
)

if(APPLE)
    target_link_libraries(sdk_test_provider_module_module_plugin PRIVATE
        "-framework CoreFoundation"
        "-framework Security"
    )
else()
    target_link_libraries(sdk_test_provider_module_module_plugin PRIVATE pthread dl)
endif()
