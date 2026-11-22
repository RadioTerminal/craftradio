# Releasing CraftRadio

A release is whatever is on the `release` branch. Pushing to it runs
`.github/workflows/release.yml`, the repository's own release workflow, adapted from SolidCraft's
(tangency-labs/solidcraft). It runs this repository's `packaging/<platform>/package.*` scripts,
signs what they produce when the signing secrets exist, and attaches it with `SHA256SUMS.txt` (which
the in-app updater reads) to a **draft** GitHub Release named **CraftRadio vX.Y.Z**. A maintainer
publishes the draft once every job is green and the checks below pass.

Signing is optional, as in SolidCraft. Each platform signs when its secrets exist in the repository's
`release` environment (only the release branch uses it) and builds unsigned, with a warning, when
they don't:

| Platform | Secrets | Without them |
|---|---|---|
| macOS | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | ad-hoc signed, not notarized |
| Windows | `WINDOWS_CERTIFICATE` and its password, or the `AZURE_*` Trusted Signing values | unsigned |
| `SHA256SUMS.txt` | `LINUX_SIGNING_KEY` (armored OpenPGP key) | no `.asc` signature |

Runners are GitHub-hosted (Ubuntu 22.04 for Linux, so the binaries need only glibc 2.35;
`macos-15`; `windows-2025` and `windows-11-arm`). A repository variable switches one to another,
for example a Blacksmith runner as SolidCraft uses: `RUNNER_LINUX`, `RUNNER_LINUX_ARM`,
`RUNNER_MACOS`, `RUNNER_WINDOWS`, `RUNNER_WINDOWS_ARM`. The release builds with Rust 1.100.0
(`RUST_TOOLCHAIN` in the workflow).

## Cutting a release

1. Bump the version on `main`: `cargo xtask version set X.Y.Z`. The workspace version in the root
   `Cargo.toml` is the only source; `cargo xtask version` prints it.
2. Fast-forward `release` to `main` and push. The run builds every platform and creates or updates
   the draft. A tag that already exists, or a release already published, stops it.
3. When every job is green, install the draft's packages on each platform and run the check below.
4. Publish the draft. GitHub writes the release notes from the merged pull requests
   (`--generate-notes`); edit them before publishing.

A dry run from any branch builds and checks every package and keeps them as the run's
`release-assets` artifact, with no release:
`gh workflow run release.yml --ref <branch> [-f version=0.1.0-rc.1]`.

## Before publishing: check the packages

On macOS, Windows and Linux, using the files attached to the draft:

1. Install or unpack the package. Run `craftradio --version` and `craftradio-cli --version` from the
   installed location; both must print the candidate version.
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
| Linux | `craftradio-<v>-linux-{x86_64,aarch64}.{AppImage,AppImage.zsync,deb,rpm,tar.gz}` |

Plus `SHA256SUMS.txt` (and `SHA256SUMS.txt.asc` when `LINUX_SIGNING_KEY` is set).

## Known gaps

- **Windows opens a console window with the app.** `craftradio.exe` is a console program so that
  `craftradio --headless` can print; `package.ps1` checks for that.
- **No Flatpak yet.** The shared Craft workflow built one; this workflow does not, though
  `packaging/linux/flatpak-bundle.sh` is still here.
- **macOS runners are billed.** GitHub-hosted macOS minutes need the account's billing in order,
  or the job never starts. `RUNNER_MACOS` can point at another provider.
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
