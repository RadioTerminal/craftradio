# MP3 output and the LGPL

The radio app explicitly enables MP3 output through LAME (LGPL). Release builds must meet the LGPL
relinking terms (AGENTS.md rule 12).

How this repository meets them: LAME is never compiled into the program. `vendor/mp3lame-sys`
replaces the upstream bindings crate's build script so the app links the LAME shared library
(`libmp3lame`) of the system, found through `LAME_LIB_DIR`, pkg-config or the default library
path (`vendor/mp3lame-sys/README.md`). Packages either depend on the distribution's LAME (deb,
rpm) or ship `libmp3lame` as a separate shared library beside the app (macOS app bundle, Windows,
AppImage, tarball), with its licence and a note on how to replace it. Replacing that file with
another build of LAME 3.100 relinks the app.

**Open blocker before a release:** the Rust crates around LAME are LGPL-3.0 too and are still
compiled into the app statically: `mp3lame-encoder` (the safe wrapper `crates/stream` uses) and the
bindings in `vendor/mp3lame-sys`. Linking LAME dynamically does not cover them. Before publishing,
either ship the object files needed to relink the app against modified copies of those crates, or
replace them with bindings under a permissive licence.

Building needs LAME installed: `brew install lame`, `sudo apt-get install libmp3lame-dev`, or on
Windows vcpkg's `mp3lame` with `LAME_LIB_DIR` and `LAME_LIB_NAME=libmp3lame`.
