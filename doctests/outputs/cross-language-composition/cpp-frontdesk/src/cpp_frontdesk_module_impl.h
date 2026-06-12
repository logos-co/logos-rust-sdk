#pragma once
#include <cstdint>
#include <string>
#include "logos_module_context.h"

// Universal (in-language-declared) C++ module consuming the Rust
// orchestrator through GENERATED typed wrappers — methods and the typed
// event subscription both come from rust_orchestrator_module's published
// .lidl contract.
class CppFrontdeskModuleImpl : public LogosModuleContext {
public:
    /// Ask the Rust orchestrator who it is (typed C++ -> Rust call).
    std::string report();

    /// Drive the full circle: C++ -> Rust tally() -> Rust -> C++ counter.
    int64_t poke(int64_t amount);

    /// Last total captured from the orchestrator's typed event.
    int64_t lastTally();

    /// Subscribe to the Rust module's typed event once the context is up.
    void onContextReady() override;

private:
    int64_t m_last = -1;
};
