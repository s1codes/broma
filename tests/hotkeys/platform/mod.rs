//! Platform-specific hotkey tests mirroring `src/hotkeys/platform/`.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;
