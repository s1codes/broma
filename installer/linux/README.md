# Broma Linux packaging

For concise commands covering these builders and the Windows installer,
see the [Makefile tutorial](../README.md).

Packagers are separated by format:

- `deb/` contains the `.deb` builder and Debian copyright file.
- `rpm/` contains the `.rpm` builder and RPM spec.
- `flatpak/` contains the Flatpak manifest, builder and generated Cargo sources.
- `appimage/` contains the AppImage build script. It uses the official
  linuxdeploy and AppImage output-plugin binaries, verifies their SHA-256 hashes,
  and assembles native x86_64 or ARM64 AppImages.
- `arm64/` contains the ARM64-only Cross configuration and Dockerfile. Its
  image installs the ARM64 ALSA development package needed by `cpal`.
- `shared/` contains the desktop entry and AppStream metadata.
- `../../assets/` contains the Windows and Linux app icons.
- `build-packages.sh` builds both native package formats for the selected CPU
  architecture.

The supported Rust targets are `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu`. Debian names these architectures `amd64` and
`arm64`; RPM names them `x86_64` and `aarch64`.

## Build Debian and RPM packages

From the repository root, build both package formats for both architectures:

```sh
./installer/linux/build-packages.sh
```

Select one architecture when building a CI matrix or a single release:

```sh
./installer/linux/build-packages.sh x86_64
./installer/linux/build-packages.sh arm64
```

The scripts use Cargo's target-specific outputs under
`target/<rust-target>/release/`. x86_64 builds use Cargo directly. ARM64 builds
use `cross` and Docker; `installer/linux/arm64/Cross.toml` selects the
ARM-specific image, which installs `libasound2-dev:arm64` for the cross-build.

To package an already-built binary separately, run
`installer/linux/deb/build.sh <x86_64|arm64>` or
`installer/linux/rpm/build.sh <x86_64|arm64>`.

Host requirements:

- Rust toolchain and Cargo
- `cross` and Docker for ARM64 Linux builds
- `dpkg-deb` and `readelf` (from `binutils`) for `.deb` packages
- `rpmbuild` for `.rpm` packages
- `libasound2-dev`, `libudev-dev`, `libwayland-dev`, `libxkbcommon-dev`, and
  `pkg-config` for compiling the Linux application
- Python 3.11+ for reading the package version from `Cargo.toml`

The Debian package declares runtime dependencies on the detected minimum
`libc6` version, `libgcc-s1`, ALSA, Wayland and XKB common. RPM's dependency
generator inspects the binary and the spec explicitly requires `alsa-lib`.

The Debian maintainer field defaults to `Broma contributors
<noreply@github.com>`. Override it when building if you want a different
contact:

```sh
DEB_MAINTAINER='Name <email@example.com>' ./installer/linux/deb/build.sh x86_64
```

## Build AppImage

Build an AppImage on a native Linux host matching the requested architecture:

```sh
# On x86_64 Linux
./installer/linux/appimage/build.sh x86_64

# On ARM64/AArch64 Linux
./installer/linux/appimage/build.sh arm64
```

The script builds the release binary, stages the desktop file, icon, AppStream
metadata and license, then bundles the runtime libraries with linuxdeploy. It
supports `x86_64` and `arm64` (AArch64); it does not cross-package, so an ARM64
build needs an ARM64 Linux host. The AppImage is written to
`dist/linux/Broma-<version>-<architecture>.AppImage`.

The build expects Cargo, `build-essential`, `curl`, `file`, `sha256sum`, and
the Linux development libraries listed for the package builds above. The
script downloads and SHA-256-verifies linuxdeploy, its output plugin, and the
AppImage runtime. These tools provide the AppDir bundler and SquashFS packer.
CI uses an Ubuntu 22.04 build container on native Ubuntu 24.04 runners as its
glibc compatibility baseline. AppImages still require a compatible glibc on the
user's Linux system.

AppImage is a Linux distribution format only. It does not provide builds for
Windows or macOS.

## Build Flatpak

Install Flatpak and `flatpak-builder`. Add Flathub as a user remote if it is
not already configured for the current user:

```sh
flatpak remote-add --user --if-not-exists flathub \
  https://dl.flathub.org/repo/flathub.flatpakrepo
```

Build Flatpaks for both architectures, or choose one:

```sh
./installer/linux/flatpak/build.sh
./installer/linux/flatpak/build.sh x86_64
./installer/linux/flatpak/build.sh arm64
```

The manifest has no architecture restriction. The builder selects the matching
Freedesktop Platform/SDK and Rust extension for branch `26.08`, then compiles
for the Rust host triple reported by that SDK. On an x86_64-only build host,
ARM64 builds need working AArch64 `binfmt_misc`/QEMU support; CI can instead use
an ARM64 runner. Outputs are written to `dist/linux/broma_<version>_<arch>.flatpak`.

When `Cargo.lock` changes, regenerate `cargo-sources.json` with the upstream
[flatpak-cargo-generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo):

```sh
python3 -m venv /tmp/flatpak-cargo-generator
/tmp/flatpak-cargo-generator/bin/pip install tomlkit aiohttp
python3 /path/to/flatpak-cargo-generator.py Cargo.lock \
  -o installer/linux/flatpak/cargo-sources.json
```

The Flatpak requests Wayland, audio-device, PulseAudio-compatible socket,
desktop portal and StatusNotifier access for the app's window, audio routing,
file picker, global-shortcut portal and tray support. It does not request
general filesystem or network access.

## Notes

- Package scripts create architecture-specific artifacts under `dist/linux/`;
  the AppImage builder requires a native host matching the requested
  architecture.
- Build `.deb` and `.rpm` artifacts against the oldest distribution baseline
  you intend to support. The Debian package records the binary's minimum glibc
  version, and RPM generates symbol dependencies from the binary.
- The `.deb` and `.rpm` packages install the binary, desktop entry, icon,
  AppStream metadata and license. They do not bundle Linux runtime libraries.
- The Flatpak manifest is suitable for local builds and CI using this checkout.
  Flathub submission should use immutable Git/archive sources rather than a
  local-directory source.
- AppStream currently omits a homepage because the repository is private.
  Add the public project URL to the metainfo before submitting the Flatpak to a
  public app store.
- The generated Cargo source list is derived from the complete workspace
  `Cargo.lock`; regenerate it whenever that lockfile changes.
