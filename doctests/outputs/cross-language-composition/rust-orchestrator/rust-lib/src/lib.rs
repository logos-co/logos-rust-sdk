//! Rust-FIRST module: the contract below is declared in Rust; the .lidl is
//! derived from it (logos-lidl-gen --from-rust) and committed next to the
//! crate — consumers in any language generate typed bindings from that.

/// The module's IPC contract. Required methods are the contract; the
/// defaulted on_context_ready hook is framework plumbing, not a method.
pub trait RustOrchestratorModule: Send + 'static {
    fn tally(&mut self, amount: i64) -> i64;
    fn whoami(&mut self) -> String;
    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}
}

/// Typed events — the Rust analog of the C++ `logos_events:` section.
pub trait RustOrchestratorModuleEvents {
    fn tally_changed(&self, total: i64);
}

include!(concat!(env!("OUT_DIR"), "/provider_gen.rs"));

#[derive(Default)]
struct Orchestrator;

impl RustOrchestratorModule for Orchestrator {
    /// Forward to the C++ counter through the typed dependency client
    /// (modules().counter — generated from cpp_counter_module's contract),
    /// then emit the typed event with the new total.
    fn tally(&mut self, amount: i64) -> i64 {
        let total = match modules().counter.increment(amount) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("orchestrator: counter.increment failed: {}", e);
                return -1;
            }
        };
        emit_tally_changed(total);
        total
    }

    /// Module context access: the host-stamped instance id.
    fn whoami(&mut self) -> String {
        context().map(|c| c.instance_id).unwrap_or_default()
    }

    fn on_context_ready(&mut self, ctx: &RustModuleContext) {
        eprintln!(
            "orchestrator ready: instance {} (persistence: {})",
            ctx.instance_id, ctx.instance_persistence_path
        );
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<Orchestrator>();
}
