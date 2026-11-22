# MP3 output and the LGPL

The radio app explicitly enables MP3 output through LAME (LGPL). Release builds must meet the LGPL
relinking terms (AGENTS.md rule 12).

How this repository meets them: no LGPL code is compiled into the program. `ffi/lame` is this
repository's own binding to LAME (MIT OR Apache-2.0). It declares the few LAME functions it calls
and links the LAME shared library (`libmp3lame`) of the system, found through `LAME_LIB_DIR`,
pkg-config or the default library path (`ffi/lame/build.rs`). Packages either depend on the
distribution's LAME (deb, rpm) or ship `libmp3lame` as a separate shared library beside the app
(macOS app bundle, Windows, AppImage, tarball, Flatpak), with LAME's licence (`LICENSE-LAME.txt`,
from `packaging/lame/COPYING`) and a note on how to replace it. Replacing that file with another
build of LAME 3.100 changes the LAME the app uses.

Building needs LAME installed: `brew install lame`, `sudo apt-get install libmp3lame-dev`, or on
Windows vcpkg's `mp3lame` with `LAME_LIB_DIR` and `LAME_LIB_NAME=libmp3lame`.

`ffi/lame` is outside the cargo workspace because FFI needs unsafe code, which the workspace
forbids (AGENTS.md rule 5). Check it on its own:

```sh
mise exec -- cargo test --manifest-path ffi/lame/Cargo.toml
mise exec -- cargo clippy --manifest-path ffi/lame/Cargo.toml --all-targets -- -D warnings
```
