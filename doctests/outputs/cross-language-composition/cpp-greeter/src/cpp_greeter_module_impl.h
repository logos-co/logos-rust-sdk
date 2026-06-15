#pragma once
#include <string>

// Hand-written, Qt-free impl of the cpp_greeter_module.lidl contract — the
// generated C-ABI wrapper + uniform Qt glue are built around this class.
class CppGreeterImpl {
public:
    std::string greet(std::string name) {
        if (name.empty()) name = "World";
        return "Hello, " + name + "! (from C++ greeter)";
    }
};
