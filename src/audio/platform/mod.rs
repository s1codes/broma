// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! OS-specific virtual microphone and audio device routing dispatch.
//!
//! Platform-specific logic is split into dedicated per-OS files so platform
//! bugs can be investigated in isolation without scrolling past unrelated OS code:
//! - [`linux`]: PipeWire / PulseAudio independent virtual mic (`module-null-sink`
//!   + `module-remap-source` + `module-loopback`) over pure-Rust Unix socket IPC.
//! - [`windows`]: WASAPI VB-Cable / VoiceMeeter discovery, feedback-loop input
//!   filtering, and lock-free `cpal` microphone passthrough.
//! - [`fallback`]: Portable fallback for future targets (e.g. macOS BlackHole).

use cpal::Device;

use crate::audio::mic_bridge::{MicBridge, MicPassthroughConsumer};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::{LinuxAudioRouter as PlatformAudioRouter, is_broma_internal_sink_name};

#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::{WindowsAudioRouter as PlatformAudioRouter, is_broma_internal_sink_name};

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub mod fallback;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use fallback::{FallbackAudioRouter as PlatformAudioRouter, is_broma_internal_sink_name};

/// Active Voice Chat / Virtual Cable output route prepared by the OS router.
/// (`None` means virtual output routing is disabled.)
pub struct ActiveVirtualRoute {
    pub sink_device: Device,
    /// Present when in-process `cpal` mic capture -> `rtrb` -> `Mixer` is used
    /// (Windows WASAPI VB-Cable, or manual external sinks).
    /// `None` when the audio server routes mic passthrough natively (Linux `module-loopback`)
    /// or when Microphone passthrough is set to `"Disabled"`.
    pub mic_bridge: Option<(MicBridge, MicPassthroughConsumer)>,
    /// Non-fatal diagnostic if e.g. the sink opened cleanly but the selected
    /// microphone was unplugged or blocked by OS privacy settings.
    pub route_warning: Option<String>,
}
