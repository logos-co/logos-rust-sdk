#include "cpp_frontdesk_module_impl.h"

// Generated umbrella: defines LogosModules (the typed dependency aggregate
// behind modules()) from metadata.json#dependencies — including the typed
// wrapper for the Rust orchestrator, generated from its published .lidl.
#include "logos_sdk.h"

std::string CppFrontdeskModuleImpl::report() {
    return modules().rust_orchestrator_module.whoami();
}

int64_t CppFrontdeskModuleImpl::poke(int64_t amount) {
    return modules().rust_orchestrator_module.tally(amount);
}

int64_t CppFrontdeskModuleImpl::lastTally() {
    return m_last;
}

void CppFrontdeskModuleImpl::onContextReady() {
    modules().rust_orchestrator_module.onTally_changed([this](int64_t total) {
        m_last = total;
    });
}
