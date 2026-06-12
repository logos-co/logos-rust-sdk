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
)

find_library(LIBRUST_PROVIDER
    NAMES librust_provider.a rust_provider
    PATHS ${CMAKE_CURRENT_SOURCE_DIR}/lib
    NO_DEFAULT_PATH
)

if(NOT LIBRUST_PROVIDER)
    message(FATAL_ERROR
        "Rust static library (librust_provider.a) not found in ${CMAKE_CURRENT_SOURCE_DIR}/lib. "
        "Ensure the preConfigure hook staged the buildRustPackage output."
    )
endif()

# The Rust archive provides the logos_module_* exports the generated glue
# calls; its own lp_* undefineds resolve against the logos-protocol archive
# already in the plugin link (via logos-qt-sdk). Re-mention the protocol
# target after the Rust archive so single-pass linkers (GNU ld) see it later
# on the line — one protocol stack shared by the glue and the Rust code.
target_link_libraries(rust_provider_module_module_plugin PRIVATE
    ${LIBRUST_PROVIDER}
    logos-protocol::logos_protocol
)

if(APPLE)
    target_link_libraries(rust_provider_module_module_plugin PRIVATE
        "-framework CoreFoundation"
        "-framework Security"
    )
else()
    target_link_libraries(rust_provider_module_module_plugin PRIVATE pthread dl)
endif()
