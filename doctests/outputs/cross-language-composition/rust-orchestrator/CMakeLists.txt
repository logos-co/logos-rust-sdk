cmake_minimum_required(VERSION 3.14)
project(RustOrchestratorModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json
               ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(
    NAME rust_orchestrator_module
)

find_library(LIBRUST_ORCHESTRATOR
    NAMES librust_orchestrator.a rust_orchestrator
    PATHS ${CMAKE_CURRENT_SOURCE_DIR}/lib
    NO_DEFAULT_PATH
)

if(NOT LIBRUST_ORCHESTRATOR)
    message(FATAL_ERROR "librust_orchestrator.a not found in lib/ — preConfigure must stage it.")
endif()

# One protocol stack shared by glue and Rust: re-mention the protocol target
# after the Rust archive so single-pass linkers resolve its lp_* references.
target_link_libraries(rust_orchestrator_module_module_plugin PRIVATE
    ${LIBRUST_ORCHESTRATOR}
    logos-protocol::logos_protocol
)

if(APPLE)
    target_link_libraries(rust_orchestrator_module_module_plugin PRIVATE
        "-framework CoreFoundation"
        "-framework Security"
    )
else()
    target_link_libraries(rust_orchestrator_module_module_plugin PRIVATE pthread dl)
endif()
