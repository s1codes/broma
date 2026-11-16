.DEFAULT_GOAL := help

ARCH ?= $(if $(filter aarch64,$(shell uname -m 2>/dev/null)),arm64,x86_64)
WINDOWS_ARCH ?= all
CARGO ?= cargo
POWERSHELL ?= pwsh
DEB_MAINTAINER ?=

export DEB_MAINTAINER CARGO_BUILD_JOBS RUSTFLAGS

.PHONY: help build test linux-packages deb rpm appimage flatpak \
        linux-installers check-linux-installers-arch windows-installer

help:
	@printf '%s\n' \
	  'Broma build and installer tasks' \
	  '' \
	  '  make build                         Build the host release binary' \
	  '  make test                          Run Rust tests' \
	  '  make linux-packages [ARCH=...]     Build Linux .deb and .rpm packages' \
	  '  make deb [ARCH=...]                Package an existing Linux binary as .deb' \
	  '  make rpm [ARCH=...]                Package an existing Linux binary as .rpm' \
	  '  make appimage [ARCH=...]           Build a native-architecture AppImage' \
	  '  make flatpak [ARCH=...]            Build Flatpak bundle(s)' \
	  '  make linux-installers [ARCH=...]   Build .deb, .rpm, AppImage, and Flatpak' \
	  '  make windows-installer [WINDOWS_ARCH=...]' \
	  '                                     Build Windows NSIS installer(s)' \
	  '' \
	  'Variables: ARCH (x86_64|arm64|all where supported), WINDOWS_ARCH,' \
	  '           DEB_MAINTAINER, CARGO_BUILD_JOBS, RUSTFLAGS, CARGO, POWERSHELL'

build:
	$(CARGO) build --locked --release

test:
	$(CARGO) test --locked

linux-packages:
	./installer/linux/build-packages.sh "$(ARCH)"

deb:
	./installer/linux/deb/build.sh "$(ARCH)"

rpm:
	./installer/linux/rpm/build.sh "$(ARCH)"

appimage:
	./installer/linux/appimage/build.sh "$(ARCH)"

flatpak:
	./installer/linux/flatpak/build.sh "$(ARCH)"

linux-installers: check-linux-installers-arch
	+$(MAKE) linux-packages ARCH="$(ARCH)"
	+$(MAKE) appimage ARCH="$(ARCH)"
	+$(MAKE) flatpak ARCH="$(ARCH)"

check-linux-installers-arch:
	@case "$(ARCH):$$(uname -m 2>/dev/null)" in \
	  x86_64:x86_64|arm64:aarch64) ;; \
	  x86_64:*|arm64:*) \
	    printf '%s\n' 'linux-installers needs ARCH to match the native host because AppImage builds are not cross-compiled.' >&2; \
	    exit 2 ;; \
	  *) \
	    printf '%s\n' 'linux-installers supports one native architecture: ARCH=x86_64 or ARCH=arm64.' >&2; \
	    exit 2 ;; \
	esac

windows-installer:
	"$(POWERSHELL)" -NoProfile -ExecutionPolicy Bypass -File installer/windows/build-installer.ps1 -Architecture "$(WINDOWS_ARCH)"
