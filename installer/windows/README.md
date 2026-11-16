# Broma Windows Installer

For Makefile shortcuts and customization examples, see the [installer
build tutorial](../README.md).

NSIS 3 builds per-user installers with architecture-specific Broma binaries:
`Broma-Setup-<version>-x64.exe` and `Broma-Setup-<version>-arm64.exe`.

## Prerequisites

- Rust toolchain with `x86_64-pc-windows-msvc` and
  `aarch64-pc-windows-msvc` targets (the build script installs a missing Rust
  target automatically).
- For Linux-hosted cross-compilation, install `cargo-xwin` with
  `cargo install cargo-xwin --locked`.
- Visual Studio C++ build tools, including the ARM64 build tools for an ARM64
  build from an x64 Windows host.
- [NSIS 3](https://nsis.sourceforge.io/) (`makensis.exe`):
  ```powershell
  winget install NSIS.NSIS
  ```

## Building the Installer

From the repository root in PowerShell, build both architectures:

```powershell
.\installer\windows\build-installer.ps1
```

Select one architecture if needed:

```powershell
.\installer\windows\build-installer.ps1 -Architecture x86_64
.\installer\windows\build-installer.ps1 -Architecture arm64
```

Each binary is read from Cargo's target-specific output directory:

```text
target\x86_64-pc-windows-msvc\release\broma.exe
target\aarch64-pc-windows-msvc\release\broma.exe
```

The installers are written to `dist\`. The NSIS setup executable is a wrapper;
its embedded Broma application binary is the selected architecture.

To cross-compile the ARM64 MSVC binary from Linux, use `cargo-xwin`, then run
NSIS with that target-specific binary:

```sh
cargo xwin build --locked --release --target aarch64-pc-windows-msvc
makensis \
  -DVERSION=0.1.0 \
  -DARCH_SUFFIX=arm64 \
  -DBIN_PATH=target/aarch64-pc-windows-msvc/release/broma.exe \
  -DAPP_ICON_PATH=assets/broma.ico \
  -DOUT_FILE=dist/Broma-Setup-0.1.0-arm64.exe \
  installer/windows/broma.nsi
```

## Replacing the Placeholder Icon

`assets/broma.ico` is a placeholder `#3B82F6` square icon containing `16x16`, `32x32`, `48x48`, and `256x256` frames.
To use custom artwork, replace `assets/broma.ico` with your multi-resolution `.ico` file and re-run `build-installer.ps1`.

## Installer Behavior

- **Per-user install** (no UAC prompt): installs to `%LOCALAPPDATA%\Programs\Broma` and registers under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Broma`.
- **Upgrades while running**: sends `taskkill /IM broma.exe` (with a force fallback if still open in the system tray) before replacing binaries or uninstalling.
- **User data preservation**: imported sounds and `library.json` live in `%LOCALAPPDATA%\broma\Broma\data`. On uninstall, Broma defaults to **No** on the optional prompt to delete user data (and leaves it intact during silent `/S` uninstalls).
