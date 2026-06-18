//! CLI: logos-lidl-gen <module.lidl> [--provider] [--dep name=dep.lidl ...]
//!                     [--protocol-version X.Y.Z] [-o out.rs]
//!
//! Default: the typed *client* backend (callers/subscribers over
//! logos_rust_sdk). `--provider`: the module-impl C ABI scaffold
//! (logos_module_* exports + typed trait + RustModuleContext) — see
//! rustgen_provider. Each `--dep name=path` appends a typed client for that
//! dependency's contract plus a `Modules` aggregate (`modules().name...`).

use std::io::Write;

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: logos-lidl-gen <module.lidl> [--provider] [--no-trait] [--dep name=dep.lidl ...] [--protocol-version X.Y.Z] [-o <out.rs>]\n\
             \x20      logos-lidl-gen --from-rust <lib.rs> --trait <ContractTrait> [--module-name n] [--module-version X.Y.Z] [-o <out.lidl>]"
        );
        std::process::exit(1);
    }

    // ── Rust-source → LIDL (the reverse direction; analog of the C++
    //    generator's --header-to-lidl) ─────────────────────────────────────
    if args.iter().any(|a| a == "--from-rust") {
        let Some(src_path) = flag_value(&args, "--from-rust") else {
            eprintln!("--from-rust requires a path to the Rust source");
            std::process::exit(1);
        };
        let Some(trait_name) = flag_value(&args, "--trait") else {
            eprintln!("--from-rust requires --trait <ContractTrait>");
            std::process::exit(1);
        };
        let module_name = flag_value(&args, "--module-name");
        let version =
            flag_value(&args, "--module-version").unwrap_or_else(|| "1.0.0".to_string());
        let source = match std::fs::read_to_string(&src_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to read {}: {}", src_path, e);
                std::process::exit(1);
            }
        };
        let module = match logos_lidl_gen::extract_from_rust(
            &source,
            &trait_name,
            module_name.as_deref(),
            &version,
        ) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Failed to extract contract from {}: {}", src_path, e);
                std::process::exit(2);
            }
        };
        let lidl = logos_lidl_gen::serialize(&module);
        match flag_value(&args, "-o") {
            Some(path) => {
                if let Err(e) = std::fs::write(&path, lidl) {
                    eprintln!("Failed to write {}: {}", path, e);
                    std::process::exit(3);
                }
                println!(
                    "Generated {} ({} methods, {} events)",
                    path,
                    module.methods.len(),
                    module.events.len()
                );
            }
            None => std::io::stdout().write_all(lidl.as_bytes()).unwrap(),
        }
        return;
    }

    let input = &args[1];
    let provider = args.iter().any(|a| a == "--provider");
    // Rust-first authoring: the trait is declared in the crate (the .lidl was
    // derived from it with --from-rust), so the provider scaffold must wrap the
    // author's trait instead of generating one. Mirrors build.rs calling
    // generate_provider_with(.., emit_trait = false).
    let no_trait = args.iter().any(|a| a == "--no-trait");
    let mut deps: Vec<(String, logos_lidl_gen::ModuleDecl)> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a != "--dep" {
            continue;
        }
        let Some(spec) = args.get(i + 1) else {
            eprintln!("--dep requires name=path");
            std::process::exit(1);
        };
        let Some((name, path)) = spec.split_once('=') else {
            eprintln!("--dep requires name=path, got: {}", spec);
            std::process::exit(1);
        };
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to read dep {}: {}", path, e);
                std::process::exit(1);
            }
        };
        match logos_lidl_gen::parse(&src) {
            Ok(m) => deps.push((name.to_string(), m)),
            Err(e) => {
                eprintln!("Failed to parse dep {}: {}", path, e);
                std::process::exit(2);
            }
        }
    }
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
    // concurrency:"multi" — emit the concurrent-dispatch scaffold (Arc<&self>
    // instance + logos_module_dispatch_async). Fed from metadata.json's
    // `concurrency` field by the builder. Absent / anything else ⇒ single.
    let multi = flag_value(&args, "--concurrency").as_deref() == Some("multi");
    let mut code = if provider {
        logos_lidl_gen::rustgen_provider::generate_provider_with(
            &module,
            &protocol_version,
            !no_trait,
            multi,
        )
    } else {
        logos_lidl_gen::generate(&module)
    };
    if !deps.is_empty() {
        code.push('\n');
        code.push_str(&logos_lidl_gen::generate_deps(&deps));
    }

    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, code) {
                eprintln!("Failed to write {}: {}", path, e);
                std::process::exit(3);
            }
            println!(
                "Generated {} ({} backend, {} methods, {} events, {} dep clients)",
                path,
                if provider { "provider" } else { "client" },
                module.methods.len(),
                module.events.len(),
                deps.len()
            );
        }
        None => {
            std::io::stdout().write_all(code.as_bytes()).unwrap();
        }
    }
}
