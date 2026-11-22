# Releasing CraftRadio

A release is whatever is on the `release` branch. Pushing to it runs
`.github/workflows/release.yml`, which calls the shared release workflow of the Craft apps
(`storytold/craft-common-actions/.github/workflows/release.yml@main`, the same one
[craft-anything](https://github.com/storytold/craft-anything) uses). That workflow runs this
repository's own `packaging/<platform>/package.*` scripts, signs what they produce and attaches it
to a **draft** GitHub Release named **CraftRadio vX.Y.Z**. A maintainer publishes the draft once
every job is green and the checks below pass.

Signing needs the maintainers' certificates, stored as secrets of the repository's `release`
environment. On the `release` branch the shared workflow refuses to build macOS or Windows without
them. A run from any other branch builds unsigned packages.

> [!IMPORTANT]
> **Nothing has been released yet.** The workflow and scripts are in place, but no run of
> `release.yml` has happened, the `release` environment has no secrets, and only
> `packaging/macos/package.sh` has been run (locally, ad-hoc signed). Treat the first run as a test
> (`gh workflow run release.yml --ref <branch> -f version=0.1.0-rc.1`).

## Cutting a release

1. Bump the version on `main`: `cargo xtask version set X.Y.Z`. The workspace version in the root
   `Cargo.toml` is the only source; `cargo xtask version` prints it.
2. Fast-forward `release` to `main` and push. The run builds every platform and creates or updates
   the draft.
3. When every job is green, install the draft's packages on each platform and run the check below.
4. Publish the draft. GitHub writes the release notes from the merged pull requests
   (`gh release create --generate-notes` in the shared workflow); edit them before publishing.

A dry run of the unsigned platforms from any branch: `gh workflow run release.yml --ref <branch>`.

## Before publishing: check the packages

On macOS, Windows and Linux, using the files attached to the draft:

1. Install or unpack the package. Run `craftradio --version` and `craftradio-cli --version` from the
   installed location; both must print the candidate version. The app only starts if it finds its
   LAME library, so this also checks that LAME shipped.
2. Open the app, load a station and play an item to the sound card.
3. Start Icecast output with MP3 to a test server and listen to the stream.

Record each platform's file name, version, OS version and the result in the release notes or
the release pull request. A platform that was not checked stays pending, and the release stays a
draft.

## What it ships

| Platform | Files |
|---|---|
| macOS | `craftradio-<v>-macos-universal.dmg` (`CraftRadio.app`), `craftradio-cli-<v>-macos-universal.zip` |
| Windows | `craftradio-<v>-windows-{x64,arm64}.msi` and `-portable.zip` |
| Linux | `craftradio-<v>-linux-{x86_64,aarch64}.{AppImage,AppImage.zsync,deb,rpm,tar.gz,flatpak}` |

Plus `SHA256SUMS.txt` (and its signature when `LINUX_SIGNING_KEY` is set).

The shared workflow can also build FreeBSD, Arch Linux, riscv64 and WebAssembly; `release.yml`
turns those off.

## MP3 and LAME

LAME is LGPL, so it is never compiled into CraftRadio (AGENTS.md rule 12, `docs/mp3-release.md`).
`vendor/mp3lame-sys` links the LAME shared library, and each package gets it like this:

| Package | LAME |
|---|---|
| macOS app | `package.sh` builds LAME 3.100 from its checked source for both architectures into `CraftRadio.app/Contents/Frameworks/libmp3lame.0.dylib`; the app finds it through its rpath |
| Windows MSI and zip | `package.ps1` builds vcpkg's `mp3lame` as `libmp3lame.dll` (static C runtime) next to `craftradio.exe` |
| deb, rpm | depend on the distribution's `libmp3lame0` / `libmp3lame.so.0` |
| AppImage, tar.gz, Flatpak | carry the build host's `libmp3lame.so.0` in `lib/craftradio/`; the app's RUNPATH is `$ORIGIN/../lib/craftradio` |

Every bundled copy sits next to `LICENSE-LAME.txt` (LAME's own `COPYING`, kept in
`packaging/lame/`) and `LAME.txt`, which says how to replace it.

> [!WARNING]
> **Release blocker:** the Rust wrapper `mp3lame-encoder` and the bindings in `vendor/mp3lame-sys`
> are LGPL-3.0 and still compiled into the app statically. See `docs/mp3-release.md`.
The macOS CLI zip has no LAME: only the app links it.

## Known gaps

- **Windows opens a console window with the app.** `craftradio.exe` is a console program so that
  `craftradio --headless` can print; `package.ps1` checks for that.
- **vcpkg is not pinned.** The Windows runners' vcpkg checkout decides the `mp3lame` port version.
- **The macOS job runs on `macos-15-xlarge`**, a paid larger runner. RadioTerminal needs it enabled
  (and billed) or the job waits for a runner that never comes.
- **No CI gate yet.** There is no `ci.yml`; run the checks in `AGENTS.md` before a release.

## App-specific choices

- **Ids.** The bundle id, desktop file, AppStream id and Flatpak id are
  `io.github.radioterminal.craftradio`. The window's Wayland app id matches it
  (`apps/radio/src/main.rs`).
- **Program names.** The cargo packages are `radio` and `radio-cli`; the programs they build are
  `craftradio` and `craftradio-cli`.
- **Icons** are rendered from `assets/app-icon/craftradio.svg` by `packaging/icons.sh` and committed.
- **The WiX UpgradeCode** `7BAF572C-F1CD-4E87-98BA-F14D81C8A6E5` belongs to CraftRadio for good.
- **Flatpak** grants the PulseAudio socket (sound output and live input), the network (Icecast and
  streams) and the home folder (stations, libraries and logs live where the person keeps them).
