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
    INCLUDE_DIRS
        ${CMAKE_CURRENT_SOURCE_DIR}/lib
)

find_library(LIBSDK_TEST_PROVIDER
    NAMES libsdk_test_provider.a sdk_test_provider
    PATHS ${CMAKE_CURRENT_SOURCE_DIR}/lib
    NO_DEFAULT_PATH
)

if(NOT LIBSDK_TEST_PROVIDER)
    message(FATAL_ERROR
        "Rust static library (libsdk_test_provider.a) not found in ${CMAKE_CURRENT_SOURCE_DIR}/lib. "
        "Ensure the preConfigure hook ran 'cargo build --release'."
    )
endif()

target_link_libraries(sdk_test_provider_module_module_plugin PRIVATE ${LIBSDK_TEST_PROVIDER})

if(APPLE)
    target_link_libraries(sdk_test_provider_module_module_plugin PRIVATE
        "-framework CoreFoundation"
        "-framework Security"
    )
else()
    target_link_libraries(sdk_test_provider_module_module_plugin PRIVATE pthread dl)
endif()
