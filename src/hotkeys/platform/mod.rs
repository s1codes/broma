// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Per-OS global hotkey backend dispatch.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::LinuxHotkeyEngine as PlatformHotkeyEngine;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::WindowsHotkeyEngine as PlatformHotkeyEngine;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub mod fallback;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use fallback::FallbackHotkeyEngine as PlatformHotkeyEngine;
