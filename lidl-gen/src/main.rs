//! CLI: logos-lidl-gen <module.lidl> [-o out.rs]

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: logos-lidl-gen <module.lidl> [-o <out.rs>]");
        std::process::exit(1);
    }
    let input = &args[1];
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
    let code = logos_lidl_gen::generate(&module);

    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, code) {
                eprintln!("Failed to write {}: {}", path, e);
                std::process::exit(3);
            }
            println!("Generated {} ({} methods, {} events)", path, module.methods.len(), module.events.len());
        }
        None => {
            std::io::stdout().write_all(code.as_bytes()).unwrap();
        }
    }
}
