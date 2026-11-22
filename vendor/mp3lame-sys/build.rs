//! Links the LAME shared library (libmp3lame) found on the system.
//!
//! Search order:
//! 1. `LAME_LIB_DIR`: a folder holding the library (`libmp3lame.dylib`, `libmp3lame.so`, or on
//!    Windows the import library). `LAME_LIB_NAME` overrides the name to link (default
//!    `mp3lame`; vcpkg on Windows ships `libmp3lame`).
//! 2. pkg-config (`mp3lame.pc`, shipped by Homebrew and most Linux distributions).
//! 3. The linker's default search path, plus Homebrew's folders on macOS.

use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=LAME_LIB_DIR");
    println!("cargo:rerun-if-env-changed=LAME_LIB_NAME");
    if env::var("DOCS_RS").is_ok_and(|v| v == "1") {
        return;
    }
    let name = env::var("LAME_LIB_NAME").unwrap_or_else(|_| "mp3lame".to_string());
    if let Ok(dir) = env::var("LAME_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=dylib={name}");
        return;
    }
    if pkg_config::Config::new().probe("mp3lame").is_ok() {
        return;
    }
    if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "macos") {
        for dir in ["/opt/homebrew/lib", "/usr/local/lib"] {
            if Path::new(dir).join("libmp3lame.dylib").exists() {
                println!("cargo:rustc-link-search=native={dir}");
            }
        }
    }
    println!("cargo:rustc-link-lib=dylib={name}");
}
