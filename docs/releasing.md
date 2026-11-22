# Releasing CraftRadio

Releases are GitHub Releases; there is no release branch. Publishing a release or a prerelease runs
`.github/workflows/release.yml`, the repository's own release workflow, adapted from SolidCraft's
(tangency-labs/solidcraft). It checks out the release's tag, runs this repository's
`packaging/<platform>/package.*` scripts, signs what they produce when the signing secrets exist,
and attaches the files to that release, `SHA256SUMS.txt` last (the in-app updater reads it, so it
offers the release only once every file is there).

**Releases ship Windows and macOS only for now.** The Linux packages and the OpenPGP signature of
`SHA256SUMS.txt` are off. Setting the repository variable `RELEASE_LINUX` to `true` turns both back
on; the Linux job and `packaging/linux/` are kept for that.

Signing is optional, as in SolidCraft. Each platform signs when its secrets exist in the repository's
`release` environment (only a published release's run uses it) and builds unsigned, with a warning,
when they don't:

| Platform | Secrets | Without them |
|---|---|---|
| macOS | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | ad-hoc signed, not notarized; the run adds a note to the release on how to open it |
| Windows | `WINDOWS_CERTIFICATE` and its password, or the `AZURE_*` Trusted Signing values | unsigned |
| `SHA256SUMS.txt` (only with `RELEASE_LINUX`) | `LINUX_SIGNING_KEY` (armored OpenPGP key) | no `.asc` signature |

Runners are GitHub-hosted (Ubuntu 22.04 for Linux, so the binaries need only glibc 2.35;
`macos-15`; `windows-2025` and `windows-11-arm`). A repository variable switches one to another,
for example a Blacksmith runner as SolidCraft uses: `RUNNER_LINUX`, `RUNNER_LINUX_ARM`,
`RUNNER_MACOS`, `RUNNER_WINDOWS`, `RUNNER_WINDOWS_ARM`. The release builds with Rust 1.100.0
(`RUST_TOOLCHAIN` in the workflow).

## Releases and prereleases

The release tag is the version: `v1.2.0` builds 1.2.0 and `v1.2.0-rc.1` builds 1.2.0-rc.1. The run
sets it in the build (`cargo xtask version set`), so `Cargo.toml` needs no bump before a release;
the run notes when the two differ.

- **A version with a suffix** (`-rc.1`, `-beta.2`) is always a prerelease. If it was published as
  a release, the run marks it as a prerelease, so it never becomes **Latest**.
- **A version without a suffix** is a release, unless it was published as a prerelease; then it
  stays one.
- The in-app updater offers stable installs only releases that are neither marked prerelease nor
  suffixed. The README's download link, `releases/latest`, never points at a prerelease.

## Cutting a release

1. On GitHub, draft a new release with a new tag `vX.Y.Z` (or `vX.Y.Z-rc.N`) on `main`, write or
   generate the notes, tick **Set as a pre-release** for a prerelease, and publish it.
2. The run builds every platform and attaches the files. If a job fails, nothing is attached:
   re-run the failed jobs and the files are attached then (uploads replace files of the same name).
3. Install the attached packages on each platform and run the check below. Until then, keep the
   release a prerelease.

Drafts do not start the workflow; only publishing does.

A dry run from any branch builds and checks every package and keeps them as the run's
`release-assets` artifact, with no release:
`gh workflow run release.yml --ref <branch> [-f version=0.1.0-rc.1]`.

## After publishing: check the packages

On macOS and Windows (and Linux when it is built), using the files attached to the release:

1. Install or unpack the package. Run `craftradio --version` and `craftradio-cli --version` from the
   installed location; both must print the candidate version.
2. Open the app, load a station and play an item to the sound card.
3. Start Icecast output with MP3 to a test server and listen to the stream.

Record each platform's file name, version, OS version and the result in the release notes or
the release pull request. A platform that was not checked stays pending, and the release stays a
prerelease.

## What it ships

| Platform | Files |
|---|---|
| macOS | `craftradio-<v>-macos-universal.dmg` (`CraftRadio.app`), `craftradio-cli-<v>-macos-universal.zip` |
| Windows | `craftradio-<v>-windows-{x64,arm64}.msi` and `-portable.zip` |
| Linux (only with `RELEASE_LINUX`) | `craftradio-<v>-linux-{x86_64,aarch64}.{AppImage,AppImage.zsync,deb,rpm,tar.gz}` |

Plus `SHA256SUMS.txt` (and `SHA256SUMS.txt.asc` when `LINUX_SIGNING_KEY` is set).

## Known gaps

- **On Windows, `craftradio --headless` prints only when its output is redirected.** The app is a
  windowed program so that no console window opens with it; in PowerShell,
  `craftradio --headless | Out-Host` shows its output. `package.ps1` checks the subsystems.
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
