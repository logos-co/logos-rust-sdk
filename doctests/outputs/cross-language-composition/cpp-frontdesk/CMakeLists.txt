cmake_minimum_required(VERSION 3.14)
project(CppFrontdeskModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

logos_module(
    NAME cpp_frontdesk_module
    SOURCES
        src/cpp_frontdesk_module_impl.h
        src/cpp_frontdesk_module_impl.cpp
)
