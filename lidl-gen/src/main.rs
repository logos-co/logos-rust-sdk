//! CLI: logos-lidl-gen <module.lidl> [--provider] [--protocol-version X.Y.Z] [-o out.rs]
//!
//! Default: the typed *client* backend (callers/subscribers over
//! logos_rust_sdk). `--provider`: the module-impl C ABI scaffold
//! (logos_module_* exports + typed trait + RustModuleContext) — see
//! rustgen_provider.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: logos-lidl-gen <module.lidl> [--provider] [--protocol-version X.Y.Z] [-o <out.rs>]"
        );
        std::process::exit(1);
    }
    let input = &args[1];
    let provider = args.iter().any(|a| a == "--provider");
    let protocol_version = args
        .iter()
        .position(|a| a == "--protocol-version")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .or_else(|| std::env::var("LOGOS_PROTOCOL_VERSION").ok())
        .unwrap_or_else(|| "0.1.0".to_string());
    let output = args
        .iter()
        .position(|a| a == "-o")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let source = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {}: {}", input, e);
            std::process::exit(1);
        }
    };
    let module = match logos_lidl_gen::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to parse {}: {}", input, e);
            std::process::exit(2);
        }
    };
    let code = if provider {
        logos_lidl_gen::generate_provider(&module, &protocol_version)
    } else {
        logos_lidl_gen::generate(&module)
    };

    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, code) {
                eprintln!("Failed to write {}: {}", path, e);
                std::process::exit(3);
            }
            println!(
                "Generated {} ({} backend, {} methods, {} events)",
                path,
                if provider { "provider" } else { "client" },
                module.methods.len(),
                module.events.len()
            );
        }
        None => {
            std::io::stdout().write_all(code.as_bytes()).unwrap();
        }
    }
}
