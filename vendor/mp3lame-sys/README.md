# mp3lame-sys (vendored, dynamically linked)

The bindings of [mp3lame-sys 0.2.0](https://github.com/DoumanAsh/mp3lame-sys) (LGPL-3.0, see
`LICENSE`) with a different build script. Upstream compiles LAME from source and links it into
the program statically. This copy links the LAME shared library of the system instead, so the
person who has a release can swap in their own build of LAME, as the LGPL requires
(AGENTS.md rule 12, `docs/mp3-release.md`).

`src/lib.rs` is the upstream file, unchanged. It only declares functions that LAME 3.100 exports.

Install LAME to build:

| Platform | Command |
|---|---|
| macOS | `brew install lame` |
| Debian/Ubuntu | `sudo apt-get install libmp3lame-dev` |
| Fedora | `sudo dnf install lame-devel` |
| Windows | `vcpkg install mp3lame:x64-windows`, then set `LAME_LIB_DIR` to its `lib` folder and `LAME_LIB_NAME=libmp3lame` |

`build.rs` documents the search order.
