# Installer build tutorial

The repository-root `Makefile` provides short commands for the existing build
and packaging scripts. Run `make help` to see the targets and variables.

## Common workflows

Build all Linux installer formats for the machine's architecture:

```sh
make linux-installers
```

This runs the Debian/RPM builder, the native AppImage builder, and the Flatpak
builder in sequence. On an ARM64 machine, the default architecture is detected
as `arm64`; otherwise it defaults to `x86_64`.

Run one task at a time:

```sh
make linux-packages          # build .deb and .rpm
make appimage                # native AppImage
make flatpak                 # Flatpak bundle
make build                   # host release binary
make deb                     # package the existing release binary
make rpm                     # package the existing release binary
make test                    # Rust tests
```

The `deb` and `rpm` targets package a binary that is already present under
`target/<rust-target>/release/`; they do not compile it. Use
`make linux-packages` for the normal build-and-package flow.

## Choose architectures and customize builds

Override `ARCH` on the command line:

```sh
make linux-packages ARCH=x86_64
make linux-packages ARCH=arm64
make linux-packages ARCH=all
make flatpak ARCH=all
```

`ARCH=all` is supported by the Linux package and Flatpak scripts, but not by
AppImage. AppImage must be built on a native host matching its architecture, so
`make linux-installers ARCH=...` accepts one architecture and checks that it
matches the host. On x86_64, an ARM64 AppImage must be built on an ARM64 host;
use CI's native ARM64 runner or run the AppImage command on an ARM64 machine.

Set the Debian maintainer field without editing the package script:

```sh
make linux-packages ARCH=x86_64 \
  DEB_MAINTAINER='Your Name <you@example.com>'
```

The Windows NSIS target is separate and must be run on Windows with the MSVC
build tools and NSIS installed:

```powershell
make windows-installer
make windows-installer WINDOWS_ARCH=x86_64
make windows-installer WINDOWS_ARCH=arm64
```

If PowerShell is installed under a different command name, override it, for
example `make windows-installer POWERSHELL=powershell`.

`CARGO` selects the executable for the `build` and `test` targets. Cargo tuning
variables are also passed through to the packaging scripts. For example, limit
parallel compilation or provide custom Rust compiler flags:

```sh
make linux-packages ARCH=x86_64 CARGO_BUILD_JOBS=4
make build RUSTFLAGS='-C target-cpu=native'
```

Use custom `RUSTFLAGS` cautiously for distributable builds: target-specific
flags can make binaries less portable. The default Makefile does not change the
Cargo release profile or build flags.

Build outputs continue to go to the existing locations: `dist/linux/` for
Linux packages and bundles, and `dist/` for Windows installers. The Makefile
only coordinates the scripts; each script remains the source of truth for
packaging behavior and its required tools.
