cmake_minimum_required(VERSION 3.14)
project(RustProviderModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json
               ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(
    NAME rust_provider_module
    INCLUDE_DIRS
        ${CMAKE_CURRENT_SOURCE_DIR}/lib
)

find_library(LIBRUST_PROVIDER
    NAMES librust_provider.a rust_provider
    PATHS ${CMAKE_CURRENT_SOURCE_DIR}/lib
    NO_DEFAULT_PATH
)

if(NOT LIBRUST_PROVIDER)
    message(FATAL_ERROR
        "Rust static library (librust_provider.a) not found in ${CMAKE_CURRENT_SOURCE_DIR}/lib. "
        "Ensure the preConfigure hook ran 'cargo build --release'."
    )
endif()

message(STATUS "Found Rust provider static library: ${LIBRUST_PROVIDER}")
target_link_libraries(rust_provider_module_module_plugin PRIVATE ${LIBRUST_PROVIDER})

if(APPLE)
    target_link_libraries(rust_provider_module_module_plugin PRIVATE
        "-framework CoreFoundation"
        "-framework Security"
    )
else()
    target_link_libraries(rust_provider_module_module_plugin PRIVATE pthread dl)
endif()
