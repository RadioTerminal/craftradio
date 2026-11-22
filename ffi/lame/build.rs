//! Links the LAME shared library (libmp3lame) of the system.
//!
//! First, `LAME_LIB_DIR` names a folder with the library, or its import library on Windows.
//! `LAME_LIB_NAME` changes the name to link. The default is `mp3lame`.
//!
//! Next, pkg-config finds `mp3lame.pc`, from Homebrew or a Linux distribution.
//!
//! Last, the linker uses its default path, and the Homebrew folders on macOS.

use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=LAME_LIB_DIR");
    println!("cargo:rerun-if-env-changed=LAME_LIB_NAME");
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
