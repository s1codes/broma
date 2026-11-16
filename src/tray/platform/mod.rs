// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Per-OS system tray construction dispatch.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::build_platform_tray;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::build_platform_tray;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub mod fallback;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use fallback::build_platform_tray;
