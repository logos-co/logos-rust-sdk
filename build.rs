//! Build script for logos-rust-sdk
//!
//! This script configures linking to:
//! - liblogos_core (from logos-liblogos)
//! - Qt libraries (Core, Network, RemoteObjects) - required by liblogos_core

use std::env;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let is_macos = target_os == "macos";

    // Get liblogos root from environment or use default paths
    let liblogos_root = env::var("LOGOS_LIBLOGOS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // Try common relative paths for local development
            let candidates = vec![
                PathBuf::from("../vendor/logos-liblogos"),
                PathBuf::from("../../logos-liblogos"),
                PathBuf::from("../../../logos-liblogos"),
            ];
            for candidate in candidates {
                if candidate.join("lib").exists() {
                    return candidate;
                }
            }
            PathBuf::from("../vendor/logos-liblogos")
        });

    println!("cargo:rerun-if-env-changed=LOGOS_LIBLOGOS_ROOT");
    println!("cargo:rerun-if-env-changed=QT_FRAMEWORK_PATH");

    // Add library search path for liblogos_core
    let lib_path = liblogos_root.join("lib");
    if lib_path.exists() {
        println!("cargo:rustc-link-search=native={}", lib_path.display());
    }

    // Link against liblogos_core (dynamic library)
    println!("cargo:rustc-link-lib=logos_core");

    // Link Qt - different approach for macOS (frameworks) vs Linux (libraries)
    if is_macos {
        // On macOS, Qt uses frameworks
        // Framework search paths are set via -F flag
        
        // In Nix builds, QT_FRAMEWORK_PATH contains the paths to Qt frameworks
        // We need to add these paths unconditionally (don't check exists() as
        // the paths are valid in the Nix sandbox but may not exist at build.rs time)
        if let Ok(qt_paths) = env::var("QT_FRAMEWORK_PATH") {
            for path in qt_paths.split(':') {
                if !path.is_empty() {
                    println!("cargo:rustc-link-arg=-F{}", path);
                }
            }
        } else {
            // Fallback to common Homebrew locations for local development
            let fallback_paths = vec![
                "/opt/homebrew/opt/qt@6/lib",
                "/usr/local/opt/qt@6/lib",
            ];

            for path in fallback_paths {
                let path = PathBuf::from(path);
                if path.exists() {
                    println!("cargo:rustc-link-arg=-F{}", path.display());
                }
            }
        }

        // Link Qt frameworks (Core, Network, RemoteObjects required by logos_core)
        println!("cargo:rustc-link-lib=framework=QtCore");
        println!("cargo:rustc-link-lib=framework=QtNetwork");
        println!("cargo:rustc-link-lib=framework=QtRemoteObjects");

        // Link C++ standard library
        println!("cargo:rustc-link-lib=c++");

        // Set rpath for runtime library loading
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../lib");
        if lib_path.exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_path.display());
        }
    } else {
        // On Linux, use regular library linking
        // Try common Qt installation paths
        let qt_lib_paths = vec![
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib64",
            "/usr/lib",
        ];

        for path in qt_lib_paths {
            let path = PathBuf::from(path);
            if path.exists() {
                println!("cargo:rustc-link-search=native={}", path.display());
            }
        }

        // Link Qt libraries (Core, Network, RemoteObjects required by logos_core)
        println!("cargo:rustc-link-lib=Qt6Core");
        println!("cargo:rustc-link-lib=Qt6Network");
        println!("cargo:rustc-link-lib=Qt6RemoteObjects");

        // Link C++ standard library
        println!("cargo:rustc-link-lib=stdc++");

        // Set rpath for runtime library loading
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
        if lib_path.exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_path.display());
        }
    }
}
