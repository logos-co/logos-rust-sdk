#pragma once
#include <cstdint>

// Hand-written, Qt-free impl of the cpp_counter_module.lidl contract —
// the generated C-ABI wrapper + uniform glue are built around this class.
class CppCounterImpl {
public:
    int64_t increment(int64_t amount) {
        m_value += amount;
        return m_value;
    }
    int64_t current() { return m_value; }

private:
    int64_t m_value = 0;
};
