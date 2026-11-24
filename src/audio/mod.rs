// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Public audio engine boundary (`AGENTS.md` §3–5).
//!
//! Coordinates:
//! - Stream 1 (`monitor`): OS Default Output (Headphones/Speakers) for local
//!   clip monitoring and modal previews (never carries user mic audio -> zero echo).
//! - Stream 2 (`virtual_cable`): Voice-chat output (Linux independent virtual mic
//!   or Windows VB-Cable/VoiceMeeter), passing the user's microphone through
//!   untouched by default and mixing soundboard clips only when triggered.
//! - Lazy `symphonia` + `rubato` decoding into shared `Arc<[i16]>` buffers
//!   backed by a bounded 16 MB LRU cache (`cache.rs`).

pub mod cache;
pub mod decode;
pub mod mic_bridge;
pub mod mixer;
pub mod platform;
pub mod service;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
#[cfg(not(target_os = "windows"))]
use cpal::{BufferSize, SupportedBufferSize};
use cpal::{
    Device, DeviceId, SampleFormat, Stream, StreamConfig, SupportedStreamConfig,
    SupportedStreamConfigRange,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::audio::cache::LruClipCache;
use crate::audio::mic_bridge::{MicBridge, MicPassthroughConsumer};
use crate::audio::mixer::{Mixer, MixerCommand, StreamStatus, VoiceTag};
use crate::audio::platform::{
    ActiveVirtualRoute, PlatformAudioRouter, is_broma_internal_sink_name,
};

pub const DEVICE_SYSTEM_DEFAULT_MIC: &str = "System Default Microphone";
pub const DEVICE_SYSTEM_DEFAULT_OUTPUT: &str = "System Default Output";
pub const DEVICE_DISABLED: &str = "Disabled";

const CMD_QUEUE_CAPACITY: usize = 128;
const RETIRED_QUEUE_CAPACITY: usize = 128;
/// Replaced mic ring buffers are rare (isolated mic recovery only); 8 is ample.
const RETIRED_MIC_QUEUE_CAPACITY: usize = 8;
/// Target callback size (~10.6 ms at 48 kHz): imperceptible trigger latency without crackling.
#[cfg(not(target_os = "windows"))]
const PREFERRED_BUFFER_FRAMES: u32 = 512;

/// A stream that stays healthy this long forgets its previous recovery backoff.
const RECOVERY_SETTLE_WINDOW: Duration = Duration::from_secs(5);

/// Bounded per-stream reopen backoff so a flapping device cannot spin the UI thread.
pub(crate) struct RecoveryBackoff {
    consecutive_failures: u32,
    next_retry_at: Option<Instant>,
    last_attempt_at: Option<Instant>,
}

impl RecoveryBackoff {
    pub(crate) fn new() -> Self {
        Self {
            consecutive_failures: 0,
            next_retry_at: None,
            last_attempt_at: None,
        }
    }

    /// Pure gate: the first recovery attempt is immediate, later ones wait.
    pub(crate) fn can_attempt(&self, now: Instant) -> bool {
        self.next_retry_at.is_none_or(|retry_at| now >= retry_at)
    }

    /// True once this route has had at least one reopen attempt since the last
    /// reset. Distinguishes "device was lost" (worth retrying) from "device was
    /// never present at boot" (leave it alone until an explicit refresh).
    pub(crate) fn has_attempted(&self) -> bool {
        self.consecutive_failures > 0
    }

    /// Whether a route should be reopened: a reported fault always qualifies,
    /// while a *missing* endpoint only qualifies after a previous attempt
    /// failed. Without that gate, sessions with no audio hardware at boot (CI,
    /// headless servers, a PC whose mic is unplugged) would retry forever.
    pub(crate) fn needs_reopen(&self, faulted: bool, missing: bool) -> bool {
        faulted || (missing && self.has_attempted())
    }

    /// Records an attempt and schedules the earliest time for the next one.
    pub(crate) fn record_attempt(&mut self, now: Instant) {
        let settled = self
            .last_attempt_at
            .is_some_and(|last| now.saturating_duration_since(last) >= RECOVERY_SETTLE_WINDOW);
        if settled {
            self.reset();
        }

        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_attempt_at = Some(now);
        self.next_retry_at = Some(now + Self::delay_before_next(self.consecutive_failures));
    }

    pub(crate) fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.next_retry_at = None;
        self.last_attempt_at = None;
    }

    fn delay_before_next(attempts: u32) -> Duration {
        match attempts {
            1 => Duration::from_millis(250),
            2 => Duration::from_millis(500),
            3 => Duration::from_millis(1_000),
            _ => Duration::from_millis(2_000),
        }
    }
}

/// Returns `false` for transient or self-recovering `cpal::ErrorKind` reports
/// (`Xrun`, `DeviceChanged`, `RealtimeDenied`) and `true` for fatal stream errors
/// requiring endpoint teardown/reopen.
pub(crate) fn is_fatal_stream_error(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

/// Encodes a `cpal::ErrorKind` into a compact `u8` so audio error callbacks can
/// record failure reasons lock-free in an `AtomicU8`: `0` for non-fatal/none,
/// `1..=11` for the known fatal variants, and `12` ("Unknown") for future
/// `#[non_exhaustive]` variants.
pub(crate) fn encode_error_kind(kind: cpal::ErrorKind) -> u8 {
    match kind {
        cpal::ErrorKind::DeviceNotAvailable => 1,
        cpal::ErrorKind::StreamInvalidated => 2,
        cpal::ErrorKind::HostUnavailable => 3,
        cpal::ErrorKind::DeviceBusy => 4,
        cpal::ErrorKind::PermissionDenied => 5,
        cpal::ErrorKind::ResourceExhausted => 6,
        cpal::ErrorKind::UnsupportedConfig => 7,
        cpal::ErrorKind::UnsupportedOperation => 8,
        cpal::ErrorKind::InvalidInput => 9,
        cpal::ErrorKind::BackendError => 10,
        cpal::ErrorKind::Other => 11,
        cpal::ErrorKind::Xrun
        | cpal::ErrorKind::DeviceChanged
        | cpal::ErrorKind::RealtimeDenied => 0,
        _ => 12,
    }
}

/// Decodes a code produced by [`encode_error_kind`] into a static diagnostic label.
pub(crate) fn decode_error_kind_label(code: u8) -> &'static str {
    match code {
        0 => "None",
        1 => "DeviceNotAvailable",
        2 => "StreamInvalidated",
        3 => "HostUnavailable",
        4 => "DeviceBusy",
        5 => "PermissionDenied",
        6 => "ResourceExhausted",
        7 => "UnsupportedConfig",
        8 => "UnsupportedOperation",
        9 => "InvalidInput",
        10 => "BackendError",
        11 => "Other",
        _ => "Unknown",
    }
}

pub(crate) struct OutputEndpoint {
    _stream: Stream,
    device_id: Option<DeviceId>,
    sample_rate: u32,
    cmd_tx: Producer<MixerCommand>,
    retired_rx: Consumer<Arc<[i16]>>,
    retired_mic_rx: Consumer<Consumer<f32>>,
    pub(crate) status: Arc<StreamStatus>,
}

impl OutputEndpoint {
    fn reclaim_retired(&mut self) {
        while let Ok(buf) = self.retired_rx.pop() {
            drop(buf);
        }
        while let Ok(mic_rx) = self.retired_mic_rx.pop() {
            drop(mic_rx);
        }
    }

    fn send(&mut self, cmd: MixerCommand) {
        self.reclaim_retired();
        let _ = self.cmd_tx.push(cmd);
    }
}

impl Drop for OutputEndpoint {
    fn drop(&mut self) {
        self.status.clear();
        self.reclaim_retired();
    }
}

/// Enumerates selectable local monitor output devices (headphones/speakers),
/// excluding Broma's hidden internal virtual bus sinks.
pub fn list_monitor_outputs(host: &cpal::Host) -> Vec<String> {
    let mut out = vec![DEVICE_SYSTEM_DEFAULT_OUTPUT.to_string()];
    if let Ok(devices) = host.output_devices() {
        for dev in devices {
            let name = device_display_name(&dev);
            if !name.is_empty()
                && !is_broma_internal_sink_name(&name)
                && name != DEVICE_SYSTEM_DEFAULT_OUTPUT
                && name != DEVICE_DISABLED
                && !out.iter().any(|existing| existing == &name)
            {
                out.push(name);
            }
        }
    }
    out.push(DEVICE_DISABLED.to_string());
    out
}

/// Resolved startup state returned alongside `AudioEngine` so `App` gets
/// enumerated device lists and sanitized selections in one pass.
pub struct AudioBootstrap {
    pub engine: AudioEngine,
    pub microphones: Vec<String>,
    pub virtual_outputs: Vec<String>,
    pub monitor_outputs: Vec<String>,
    pub selected_microphone: String,
    pub selected_virtual_output: String,
    pub selected_monitor: String,
}

pub struct AudioEngine {
    // Field order matters for `Drop`: streams close BEFORE `router` tears down
    // any managed PipeWire/PulseAudio virtual microphone modules.
    pub(crate) monitor: Option<OutputEndpoint>,
    virtual_cable: Option<OutputEndpoint>,
    mic_bridge: Option<MicBridge>,
    router: PlatformAudioRouter,

    microphone_selection: String,
    virtual_output_selection: String,
    monitor_selection: String,
    global_volume: f32,
    monitor_volume: f32,
    cable_volume: f32,
    pub(crate) cache: LruClipCache,
    pub(crate) last_device_error: Option<String>,

    monitor_backoff: RecoveryBackoff,
    cable_backoff: RecoveryBackoff,
    mic_backoff: RecoveryBackoff,
    /// Error waiting to be surfaced to the UI exactly once.
    pending_device_error: Option<String>,
    /// Last error text already queued, so a flapping stream does not spam the UI.
    last_reported_error: Option<String>,
}

impl AudioEngine {
    /// Initializes the platform router, enumerates microphones and virtual outputs,
    /// sanitizes saved config selections, and opens the default passthrough + monitor streams.
    pub fn bootstrap(
        saved_microphone: Option<&str>,
        saved_virtual_output: Option<&str>,
        saved_monitor_output: Option<&str>,
        global_volume_percent: u8,
        monitor_volume_percent: u8,
        cable_volume_percent: u8,
    ) -> AudioBootstrap {
        let host = cpal::default_host();
        let router = PlatformAudioRouter::new();

        let microphones = router.list_microphones(&host);
        let virtual_outputs = router.list_virtual_outputs(&host);
        let monitor_outputs = list_monitor_outputs(&host);

        let selected_microphone = saved_microphone
            .filter(|m| microphones.iter().any(|item| item == m))
            .map(str::to_string)
            .unwrap_or_else(|| DEVICE_SYSTEM_DEFAULT_MIC.to_string());

        let selected_virtual_output = saved_virtual_output
            .filter(|o| virtual_outputs.iter().any(|item| item == o))
            .map(str::to_string)
            .unwrap_or_else(|| router.default_virtual_output(&virtual_outputs));

        let selected_monitor = saved_monitor_output
            .filter(|m| monitor_outputs.iter().any(|item| item == m))
            .map(str::to_string)
            .unwrap_or_else(|| DEVICE_SYSTEM_DEFAULT_OUTPUT.to_string());

        let mut engine = Self {
            monitor: None,
            virtual_cable: None,
            mic_bridge: None,
            router,
            microphone_selection: selected_microphone.clone(),
            virtual_output_selection: selected_virtual_output.clone(),
            monitor_selection: selected_monitor.clone(),
            global_volume: volume_to_linear(global_volume_percent),
            monitor_volume: volume_to_linear(monitor_volume_percent),
            cable_volume: volume_to_linear(cable_volume_percent),
            cache: LruClipCache::default(),
            last_device_error: None,
            monitor_backoff: RecoveryBackoff::new(),
            cable_backoff: RecoveryBackoff::new(),
            mic_backoff: RecoveryBackoff::new(),
            pending_device_error: None,
            last_reported_error: None,
        };
        engine.reopen_streams();

        AudioBootstrap {
            engine,
            microphones,
            virtual_outputs,
            monitor_outputs,
            selected_microphone,
            selected_virtual_output,
            selected_monitor,
        }
    }

    pub fn set_microphone(&mut self, selection: &str) {
        if self.microphone_selection == selection {
            return;
        }
        self.stop_all();
        self.microphone_selection = selection.to_string();
        self.reset_recovery_state();
        self.reopen_streams();
    }

    pub fn set_virtual_output(&mut self, selection: &str) {
        if self.virtual_output_selection == selection {
            return;
        }
        self.stop_all();
        self.virtual_output_selection = selection.to_string();
        self.reset_recovery_state();
        self.reopen_streams();
    }

    pub fn set_monitor_output(&mut self, selection: &str) {
        if self.monitor_selection == selection {
            return;
        }
        self.stop_all();
        self.monitor_selection = selection.to_string();
        self.reset_recovery_state();
        self.reopen_streams();
    }

    /// An explicit device change is a fresh start: forget prior recovery backoff.
    fn reset_recovery_state(&mut self) {
        self.monitor_backoff.reset();
        self.cable_backoff.reset();
        self.mic_backoff.reset();
        self.last_reported_error = None;
    }

    pub fn set_global_volume(&mut self, global_volume_percent: u8) {
        let linear = volume_to_linear(global_volume_percent);
        self.global_volume = linear;
        if let Some(m) = &mut self.monitor {
            m.send(MixerCommand::SetGlobalVolume(linear));
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(MixerCommand::SetGlobalVolume(linear));
        }
    }

    pub fn set_monitor_volume(&mut self, monitor_volume_percent: u8) {
        let linear = volume_to_linear(monitor_volume_percent);
        self.monitor_volume = linear;
        if let Some(m) = &mut self.monitor {
            m.send(MixerCommand::SetStreamVolume(linear));
        }
    }

    pub fn set_cable_volume(&mut self, cable_volume_percent: u8) {
        let linear = volume_to_linear(cable_volume_percent);
        self.cable_volume = linear;
        if let Some(c) = &mut self.virtual_cable {
            c.send(MixerCommand::SetStreamVolume(linear));
        }
    }

    pub fn clear_cache(&mut self) {
        self.reclaim_retired_buffers();
        self.cache.clear();
    }

    pub fn refresh_device_lists(&self) -> (Vec<String>, Vec<String>, Vec<String>) {
        let host = cpal::default_host();
        (
            self.router.list_microphones(&host),
            self.router.list_virtual_outputs(&host),
            list_monitor_outputs(&host),
        )
    }

    /// Bitmask of currently playing pad slots (`bit i == 1` while slot `i` is playing).
    pub fn active_slots_mask(&self) -> u64 {
        let m = self
            .monitor
            .as_ref()
            .map_or(0, |ep| ep.status.active_slots_mask.load(Ordering::Relaxed));
        let c = self
            .virtual_cable
            .as_ref()
            .map_or(0, |ep| ep.status.active_slots_mask.load(Ordering::Relaxed));
        m | c
    }

    pub fn is_slot_playing(&self, slot_index: usize) -> bool {
        slot_index < 64 && (self.active_slots_mask() & (1u64 << slot_index)) != 0
    }

    pub fn is_previewing(&self) -> bool {
        let m = self
            .monitor
            .as_ref()
            .is_some_and(|ep| ep.status.preview_playing.load(Ordering::Relaxed));
        let c = self
            .virtual_cable
            .as_ref()
            .is_some_and(|ep| ep.status.preview_playing.load(Ordering::Relaxed));
        m || c
    }

    /// Drains any finished `Arc<[i16]>` buffers queued from the audio callbacks
    /// and recovers any stream invalidated by the OS audio backend (e.g. WASAPI unplug).
    pub fn reclaim_retired_buffers(&mut self) {
        if let Some(m) = &mut self.monitor {
            m.reclaim_retired();
        }
        if let Some(c) = &mut self.virtual_cable {
            c.reclaim_retired();
        }
        let _ = self.recover_if_faulted();
    }

    /// Checks lock-free fault flags set by `cpal` stream error callbacks (such as
    /// Windows WASAPI `AUDCLNT_E_DEVICE_INVALIDATED` on headset unplug or sample-rate
    /// change) and reopens audio routes automatically instead of staying silently dead.
    pub fn recover_if_faulted(&mut self) -> bool {
        let now = Instant::now();
        let monitor_faulted = self.is_monitor_faulted();
        let cable_faulted = self.is_cable_faulted();
        let mic_faulted = self.is_mic_faulted();

        // A route that never opened at boot is not retried here: it only becomes
        // retry-worthy after a reopen actually failed, or on an explicit
        // "Refresh Devices". This keeps headless/no-device sessions inert
        // instead of burning backoff attempts on absent hardware.
        let cable_needs = self.cable_backoff.needs_reopen(
            cable_faulted,
            self.virtual_cable.is_none() && self.cable_wanted(),
        );
        let monitor_needs = self.monitor_backoff.needs_reopen(
            monitor_faulted,
            self.monitor.is_none() && self.monitor_wanted(),
        );
        // Fueling a mic bridge with no live cable endpoint is pointless.
        let mic_missing = self.mic_bridge.is_none() && self.mic_wanted() && !cable_needs;
        let mic_needs = self.mic_backoff.needs_reopen(mic_faulted, mic_missing);

        let attempt_cable = cable_needs && self.cable_backoff.can_attempt(now);
        let attempt_monitor = monitor_needs && self.monitor_backoff.can_attempt(now);
        // Reopening the cable route already refreshes the mic bridge.
        let attempt_mic = mic_needs && !cable_needs && self.mic_backoff.can_attempt(now);

        if !attempt_cable && !attempt_monitor && !attempt_mic {
            return false;
        }

        // Retries supersede any error recorded by the previous failed attempt.
        self.last_device_error = None;

        if attempt_cable {
            self.cable_backoff.record_attempt(now);
            self.note_fault(
                "Virtual cable",
                Self::endpoint_error_code(self.virtual_cable.as_ref()),
                cable_faulted,
            );
            self.reopen_virtual_cable_only();
        }

        // Independent monitor recovery: never tears down live cable playback.
        if attempt_monitor {
            self.monitor_backoff.record_attempt(now);
            self.note_fault(
                "Monitor",
                Self::endpoint_error_code(self.monitor.as_ref()),
                monitor_faulted,
            );
            self.reopen_monitor_only();
        }

        // Mic-only recovery keeps live soundboard voices on the cable playing.
        if attempt_mic {
            self.mic_backoff.record_attempt(now);
            self.note_fault(
                "Microphone",
                self.mic_bridge
                    .as_ref()
                    .map_or(0, MicBridge::last_error_code),
                mic_faulted,
            );
            self.reopen_mic_only();
        }

        self.publish_device_error();
        true
    }

    fn is_monitor_faulted(&self) -> bool {
        self.monitor
            .as_ref()
            .is_some_and(|ep| ep.status.stream_faulted.load(Ordering::Acquire))
    }

    fn is_cable_faulted(&self) -> bool {
        self.virtual_cable
            .as_ref()
            .is_some_and(|ep| ep.status.stream_faulted.load(Ordering::Acquire))
    }

    fn is_mic_faulted(&self) -> bool {
        self.mic_bridge.as_ref().is_some_and(MicBridge::is_faulted)
    }

    fn cable_wanted(&self) -> bool {
        !self.virtual_output_selection.is_empty()
            && self.virtual_output_selection != DEVICE_DISABLED
    }

    fn monitor_wanted(&self) -> bool {
        !self.monitor_selection.is_empty() && self.monitor_selection != DEVICE_DISABLED
    }

    fn mic_wanted(&self) -> bool {
        !self.microphone_selection.is_empty() && self.microphone_selection != DEVICE_DISABLED
    }

    fn endpoint_error_code(endpoint: Option<&OutputEndpoint>) -> u8 {
        endpoint.map_or(0, |ep| ep.status.last_error_code.load(Ordering::Relaxed))
    }

    /// Queues a one-shot diagnostic for the UI, deduplicated against the last report.
    fn note_fault(&mut self, stream: &str, code: u8, faulted: bool) {
        let detail = if code != 0 {
            decode_error_kind_label(code).to_string()
        } else if faulted {
            "device changed".to_string()
        } else {
            "not available".to_string()
        };
        let state = if faulted { "faulted" } else { "unavailable" };
        let message = format!("{stream} audio stream {state} ({detail}); reopening");
        if self.last_reported_error.as_deref() != Some(message.as_str()) {
            self.pending_device_error = Some(message.clone());
            self.last_reported_error = Some(message);
        }
    }

    /// Publishes the most recent open failure if it was not already reported.
    fn publish_device_error(&mut self) {
        let Some(err) = self.last_device_error.clone() else {
            return;
        };
        if self.last_reported_error.as_deref() != Some(err.as_str()) {
            self.pending_device_error = Some(err.clone());
            self.last_reported_error = Some(err);
        }
    }

    /// Takes the pending audio error for the UI, if any.
    pub fn take_pending_device_error(&mut self) -> Option<String> {
        self.pending_device_error.take()
    }

    /// Clears stale routing errors after a successful explicit device switch.
    pub fn clear_device_error(&mut self) {
        self.pending_device_error = None;
        self.last_reported_error = None;
        self.last_device_error = None;
    }

    /// Reopens missing or faulted streams after an explicit "Refresh Devices",
    /// leaving healthy playing streams untouched.
    pub fn recover_missing_or_faulted(&mut self) {
        self.monitor_backoff.reset();
        self.cable_backoff.reset();
        self.mic_backoff.reset();
        self.last_reported_error = None;
        self.last_device_error = None;

        let cable_needs =
            self.is_cable_faulted() || (self.virtual_cable.is_none() && self.cable_wanted());
        if cable_needs {
            self.reopen_virtual_cable_only();
        } else {
            let mic_needs = self.is_mic_faulted()
                || (self.mic_bridge.is_none() && self.mic_wanted() && self.virtual_cable.is_some());
            if mic_needs {
                self.reopen_mic_only();
            }
        }

        let monitor_needs =
            self.is_monitor_faulted() || (self.monitor.is_none() && self.monitor_wanted());
        if monitor_needs {
            self.reopen_monitor_only();
        }

        self.arm_retries_for_still_missing_routes();
        self.publish_device_error();
    }

    /// A refresh that failed leaves the route missing with no fault flag, so
    /// background recovery would never run. Record a failed attempt to keep the
    /// retry loop armed for when the device later becomes available.
    fn arm_retries_for_still_missing_routes(&mut self) {
        let now = Instant::now();
        if self.virtual_cable.is_none() && self.cable_wanted() {
            self.cable_backoff.record_attempt(now);
        }
        if self.mic_bridge.is_none() && self.mic_wanted() && self.virtual_cable.is_some() {
            self.mic_backoff.record_attempt(now);
        }
        if self.monitor.is_none() && self.monitor_wanted() {
            self.monitor_backoff.record_attempt(now);
        }
    }

    pub fn invalidate_cached_file(&mut self, path: &Path) {
        self.reclaim_retired_buffers();
        self.cache.invalidate_path(path);
    }

    /// Plays `slot_index` across active output streams (Monitor + Virtual Mic/Cable).
    /// Returns the pitch-adjusted playback duration in milliseconds.
    pub fn play(
        &mut self,
        slot_index: usize,
        path: &Path,
        custom_volume: Option<u8>,
        global_volume_percent: u8,
        pitch: f32,
    ) -> Result<u64, String> {
        self.reclaim_retired_buffers();
        self.global_volume = volume_to_linear(global_volume_percent);

        if self.monitor.is_none() && self.virtual_cable.is_none() {
            return Err(self
                .last_device_error
                .clone()
                .unwrap_or_else(|| "No active audio output device available".to_string()));
        }

        let target_rate = self.preferred_sample_rate();
        let clip = self.cache.get_or_decode(path, target_rate)?;

        // A cold-cache decode can take long enough for a device to fault underneath us.
        let _ = self.recover_if_faulted();
        if self.monitor.is_none() && self.virtual_cable.is_none() {
            return Err(self
                .last_device_error
                .clone()
                .unwrap_or_else(|| "No active audio output device available".to_string()));
        }

        let follows_global = custom_volume.is_none();
        let base_gain = volume_to_linear(custom_volume.unwrap_or(global_volume_percent));
        let safe_pitch = pitch.clamp(0.25, 4.0);

        // One command per stream (`ReplaceMic` makes `MixerCommand` non-cloneable).
        let play_cmd = || MixerCommand::Play {
            tag: VoiceTag::Slot(slot_index),
            samples: Arc::clone(&clip.samples),
            clip_channels: clip.channels,
            clip_sample_rate: clip.sample_rate,
            base_gain,
            pitch: safe_pitch,
            follows_global_volume: follows_global,
        };

        if let Some(m) = &mut self.monitor {
            m.send(MixerCommand::SetGlobalVolume(self.global_volume));
            m.send(play_cmd());
            if slot_index < 64 && !m.status.stream_faulted.load(Ordering::Acquire) {
                m.status
                    .active_slots_mask
                    .fetch_or(1u64 << slot_index, Ordering::Relaxed);
            }
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(MixerCommand::SetGlobalVolume(self.global_volume));
            c.send(play_cmd());
            if slot_index < 64 && !c.status.stream_faulted.load(Ordering::Acquire) {
                c.status
                    .active_slots_mask
                    .fetch_or(1u64 << slot_index, Ordering::Relaxed);
            }
        }

        Ok(adjusted_duration_ms(clip.duration_ms(), safe_pitch))
    }

    pub fn stop(&mut self, slot_index: usize) {
        if let Some(m) = &mut self.monitor {
            m.send(MixerCommand::StopTag(VoiceTag::Slot(slot_index)));
            if slot_index < 64 {
                m.status
                    .active_slots_mask
                    .fetch_and(!(1u64 << slot_index), Ordering::Relaxed);
            }
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(MixerCommand::StopTag(VoiceTag::Slot(slot_index)));
            if slot_index < 64 {
                c.status
                    .active_slots_mask
                    .fetch_and(!(1u64 << slot_index), Ordering::Relaxed);
            }
        }
        self.reclaim_retired_buffers();
    }

    pub fn stop_all(&mut self) {
        if let Some(m) = &mut self.monitor {
            m.send(MixerCommand::StopAll);
            m.status.clear();
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(MixerCommand::StopAll);
            c.status.clear();
        }
        self.reclaim_retired_buffers();
    }

    /// Toggles playback for a pad slot: stops if currently playing, starts otherwise.
    pub fn toggle_slot(
        &mut self,
        slot_index: usize,
        path: &Path,
        custom_volume: Option<u8>,
        global_volume_percent: u8,
        pitch: f32,
    ) -> Result<Option<u64>, String> {
        self.stop_preview();
        if self.is_slot_playing(slot_index) {
            self.stop(slot_index);
            Ok(None)
        } else {
            let dur = self.play(
                slot_index,
                path,
                custom_volume,
                global_volume_percent,
                pitch,
            )?;
            Ok(Some(dur))
        }
    }

    /// Toggles modal preview on the local Headphones Monitor stream only (never
    /// sent over the Virtual Mic / VB-Cable so teammates only hear normal mic voice).
    pub fn toggle_preview(
        &mut self,
        path: &Path,
        volume_percent: u8,
        pitch: f32,
    ) -> Result<Option<u64>, String> {
        self.reclaim_retired_buffers();
        if self.is_previewing() {
            self.stop_preview();
            return Ok(None);
        }

        if self.monitor.is_none() && self.virtual_cable.is_none() {
            return Err(self
                .last_device_error
                .clone()
                .unwrap_or_else(|| "No active audio monitor output available".to_string()));
        }

        let target_rate = self.preferred_sample_rate();
        let clip = self.cache.get_or_decode(path, target_rate)?;

        // Recover any fault that surfaced while decoding before starting the preview.
        let _ = self.recover_if_faulted();
        if self.monitor.is_none() && self.virtual_cable.is_none() {
            return Err(self
                .last_device_error
                .clone()
                .unwrap_or_else(|| "No active audio monitor output available".to_string()));
        }

        let safe_pitch = pitch.clamp(0.25, 4.0);
        let gain = volume_to_linear(volume_percent);

        let cmd = MixerCommand::Play {
            tag: VoiceTag::Preview,
            samples: Arc::clone(&clip.samples),
            clip_channels: clip.channels,
            clip_sample_rate: clip.sample_rate,
            base_gain: gain,
            pitch: safe_pitch,
            follows_global_volume: false,
        };

        if let Some(m) = &mut self.monitor {
            m.send(cmd);
            if !m.status.stream_faulted.load(Ordering::Acquire) {
                m.status.preview_playing.store(true, Ordering::Relaxed);
            }
        } else if let Some(c) = &mut self.virtual_cable {
            // Only fall back if no local headphone output exists on the system.
            c.send(cmd);
            if !c.status.stream_faulted.load(Ordering::Acquire) {
                c.status.preview_playing.store(true, Ordering::Relaxed);
            }
        }

        Ok(Some(adjusted_duration_ms(clip.duration_ms(), safe_pitch)))
    }

    /// Live-updates volume and pitch while a modal preview clip is playing.
    pub fn update_preview(&mut self, volume_percent: u8, pitch: f32) {
        let preview_cmd = || MixerCommand::UpdatePreview {
            gain: volume_to_linear(volume_percent),
            pitch: pitch.clamp(0.25, 4.0),
        };
        if let Some(m) = &mut self.monitor {
            m.send(preview_cmd());
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(preview_cmd());
        }
    }

    pub fn stop_preview(&mut self) {
        if let Some(m) = &mut self.monitor {
            m.send(MixerCommand::StopTag(VoiceTag::Preview));
            m.status.preview_playing.store(false, Ordering::Relaxed);
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(MixerCommand::StopTag(VoiceTag::Preview));
            c.status.preview_playing.store(false, Ordering::Relaxed);
        }
        self.reclaim_retired_buffers();
    }

    fn preferred_sample_rate(&self) -> u32 {
        self.monitor
            .as_ref()
            .map(|ep| ep.sample_rate)
            .or_else(|| self.virtual_cable.as_ref().map(|ep| ep.sample_rate))
            .unwrap_or(48_000)
    }

    fn reopen_streams(&mut self) {
        // Close existing streams BEFORE re-provisioning platform virtual audio routes.
        self.monitor = None;
        self.virtual_cable = None;
        self.mic_bridge = None;
        self.last_device_error = None;

        self.open_virtual_route();
        self.open_monitor_only();
        self.publish_device_error();
    }

    /// Drops and reopens only the monitor endpoint, leaving cable playback intact.
    fn reopen_monitor_only(&mut self) {
        self.monitor = None;
        self.open_monitor_only();
    }

    /// Drops only the mic capture bridge and hot-swaps the live cable mixer's
    /// mic consumer, so playing soundboard voices are never interrupted.
    fn reopen_mic_only(&mut self) {
        self.mic_bridge = None;
        let host = cpal::default_host();
        let (bridge, warning) = self
            .router
            .open_mic_bridge(&host, &self.microphone_selection);

        if let Some(warn) = warning {
            self.last_device_error = Some(warn);
        }

        let Some((bridge, consumer)) = bridge else {
            return;
        };
        let Some(cable) = &mut self.virtual_cable else {
            return;
        };

        cable.send(MixerCommand::ReplaceMic(Some(consumer)));
        self.mic_bridge = Some(bridge);
    }

    /// Reopens the virtual cable route (and its mic bridge) without touching monitor.
    fn reopen_virtual_cable_only(&mut self) {
        self.virtual_cable = None;
        self.mic_bridge = None;
        self.open_virtual_route();
    }

    /// Prepares Stream 2: Voice Chat / Virtual Mic output (+ mic passthrough).
    fn open_virtual_route(&mut self) {
        let host = cpal::default_host();
        match self.router.prepare_route(
            &host,
            &self.microphone_selection,
            &self.virtual_output_selection,
        ) {
            Ok(None) => {}
            Ok(Some(ActiveVirtualRoute {
                sink_device,
                mic_bridge,
                route_warning,
            })) => {
                if let Some(warn) = route_warning {
                    self.last_device_error = Some(warn);
                }
                let (bridge_handle, mic_consumer) = match mic_bridge {
                    Some((b, c)) => (Some(b), Some(c)),
                    None => (None, None),
                };
                match open_endpoint(sink_device, self.global_volume, mic_consumer) {
                    Ok(mut ep) => {
                        ep.send(MixerCommand::SetStreamVolume(self.cable_volume));
                        self.virtual_cable = Some(ep);
                        self.mic_bridge = bridge_handle;
                    }
                    Err(err) => {
                        self.last_device_error = Some(err);
                    }
                }
            }
            Err(err) => {
                self.last_device_error = Some(err);
            }
        }
    }

    /// Opens Stream 1: Local Headphones/Speakers Monitor, ensuring we never
    /// bind Monitor to the Virtual Cable / internal bus itself.
    fn open_monitor_only(&mut self) {
        let host = cpal::default_host();
        let cable_id = self
            .virtual_cable
            .as_ref()
            .and_then(|c| c.device_id.as_ref());

        if let Some(monitor_dev) =
            resolve_local_monitor_device(&host, &self.monitor_selection, cable_id)
        {
            match open_endpoint(monitor_dev, self.global_volume, None) {
                Ok(mut ep) => {
                    ep.send(MixerCommand::SetStreamVolume(self.monitor_volume));
                    self.monitor = Some(ep);
                }
                Err(err) => {
                    if self.last_device_error.is_none() {
                        self.last_device_error = Some(err);
                    }
                }
            }
        }
    }
}

pub(crate) fn device_display_name(device: &Device) -> String {
    device
        .description()
        .map(|d| d.name().trim().to_string())
        .unwrap_or_else(|_| device.to_string().trim().to_string())
}

fn resolve_local_monitor_device(
    host: &cpal::Host,
    monitor_selection: &str,
    exclude_cable_id: Option<&DeviceId>,
) -> Option<Device> {
    if monitor_selection.is_empty() || monitor_selection == DEVICE_DISABLED {
        return None;
    }

    if monitor_selection != DEVICE_SYSTEM_DEFAULT_OUTPUT {
        let mut outputs = host.output_devices().ok()?;
        return outputs.find(|dev| {
            let name = device_display_name(dev);
            let id_conflicts =
                exclude_cable_id.is_some() && dev.id().ok().as_ref() == exclude_cable_id;
            name == monitor_selection && !is_broma_internal_sink_name(&name) && !id_conflicts
        });
    }

    if let Some(default_out) = host.default_output_device() {
        // Windows: prefer the concrete enumerated handle over the default endpoint (#1402).
        #[cfg(target_os = "windows")]
        let default_out = crate::audio::platform::windows::resolve_specific_device(
            default_out,
            host.output_devices().ok(),
        );
        let name = device_display_name(&default_out);
        let id_conflicts =
            exclude_cable_id.is_some() && default_out.id().ok().as_ref() == exclude_cable_id;
        if !is_broma_internal_sink_name(&name) && !id_conflicts {
            return Some(default_out);
        }
    }

    // Fallback if the user set OS default output itself to VB-Cable / Broma bus:
    // pick the first real hardware output so local monitoring stays separate.
    let mut outputs = host.output_devices().ok()?;
    outputs.find(|dev| {
        let name = device_display_name(dev);
        let id_conflicts = exclude_cable_id.is_some() && dev.id().ok().as_ref() == exclude_cable_id;
        !name.is_empty() && !is_broma_internal_sink_name(&name) && !id_conflicts
    })
}

#[inline]
fn volume_to_linear(percent: u8) -> f32 {
    f32::from(percent.min(100)) / 100.0
}

fn adjusted_duration_ms(base_ms: u64, pitch: f32) -> u64 {
    let scaled = (base_ms as f64) / f64::from(pitch.max(0.25));
    (scaled.ceil() as u64).max(40)
}

/// Ranks all supported PCM stream configurations, avoiding low-rate traps (e.g. 8 kHz
/// entries listed first on Windows WASAPI) by scoring sample rate, channel count, and format.
pub(crate) fn select_best_supported_config<I>(
    ranges: I,
    preferred_channels: u16,
) -> Option<SupportedStreamConfig>
where
    I: IntoIterator<Item = SupportedStreamConfigRange>,
{
    let mut best: Option<((u8, u32, u8, u8), SupportedStreamConfig)> = None;

    for range in ranges {
        if !matches!(
            range.sample_format(),
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
        ) {
            continue;
        }
        if range.min_sample_rate() > range.max_sample_rate() {
            continue;
        }

        let rate = 48_000.clamp(range.min_sample_rate(), range.max_sample_rate());
        let candidate = range.with_sample_rate(rate);

        let (rate_tier, rate_subscore) = match rate {
            48_000 => (3u8, u32::MAX),
            44_100 => (2u8, u32::MAX),
            r if r >= 44_100 => (1u8, u32::MAX - r.abs_diff(48_000)),
            r => (0u8, r),
        };
        let channel_match = u8::from(candidate.channels() == preferred_channels);
        let format_score = match candidate.sample_format() {
            SampleFormat::F32 => 3u8,
            SampleFormat::I16 => 2u8,
            SampleFormat::U16 => 1u8,
            _ => 0u8,
        };
        let score = (rate_tier, rate_subscore, channel_match, format_score);

        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score > *best_score)
        {
            best = Some((score, candidate));
        }
    }

    best.map(|(_, cfg)| cfg)
}

fn select_output_config(device: &Device) -> Result<SupportedStreamConfig, String> {
    if let Ok(default_cfg) = device.default_output_config()
        && matches!(
            default_cfg.sample_format(),
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
        )
    {
        return Ok(default_cfg);
    }

    let ranges = device
        .supported_output_configs()
        .map_err(|e| format!("Failed to query device configs: {e}"))?;

    select_best_supported_config(ranges, 2)
        .ok_or_else(|| "No supported PCM format on output device".to_string())
}

fn open_endpoint(
    device: Device,
    initial_global_volume: f32,
    mic_passthrough: Option<MicPassthroughConsumer>,
) -> Result<OutputEndpoint, String> {
    let supported = select_output_config(&device)?;
    let sample_format = supported.sample_format();
    let sample_rate = supported.sample_rate();
    let channels = supported.channels();
    let device_id = device.id().ok();

    #[cfg(target_os = "windows")]
    {
        // Keep `BufferSize::Default` on WASAPI so Windows allocates its safe
        // multi-period shared-mode buffer while firing callbacks at device period.
        let default_cfg: StreamConfig = supported.into();
        build_typed_stream(
            &device,
            default_cfg,
            sample_format,
            channels,
            sample_rate,
            device_id,
            initial_global_volume,
            mic_passthrough,
        )
        .map_err(|(err, _)| err)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut low_latency_cfg: StreamConfig = supported.into();
        if let SupportedBufferSize::Range { min, max } = supported.buffer_size() {
            let frames = PREFERRED_BUFFER_FRAMES.clamp(*min, *max);
            low_latency_cfg.buffer_size = BufferSize::Fixed(frames);
        }

        match build_typed_stream(
            &device,
            low_latency_cfg,
            sample_format,
            channels,
            sample_rate,
            device_id.clone(),
            initial_global_volume,
            mic_passthrough,
        ) {
            Ok(ep) => Ok(ep),
            Err((_, recovered_mic)) if low_latency_cfg.buffer_size != BufferSize::Default => {
                let default_cfg: StreamConfig = supported.into();
                build_typed_stream(
                    &device,
                    default_cfg,
                    sample_format,
                    channels,
                    sample_rate,
                    device_id,
                    initial_global_volume,
                    recovered_mic,
                )
                .map_err(|(err, _)| err)
            }
            Err((err, _)) => Err(err),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_typed_stream(
    device: &Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    channels: u16,
    sample_rate: u32,
    device_id: Option<DeviceId>,
    initial_global_volume: f32,
    mic_passthrough: Option<MicPassthroughConsumer>,
) -> Result<OutputEndpoint, (String, Option<MicPassthroughConsumer>)> {
    if !matches!(
        sample_format,
        SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
    ) {
        return Err((
            format!("Unsupported device sample format: {sample_format:?}"),
            mic_passthrough,
        ));
    }

    let (cmd_tx, cmd_rx) = RingBuffer::new(CMD_QUEUE_CAPACITY);
    let (retired_tx, retired_rx) = RingBuffer::new(RETIRED_QUEUE_CAPACITY);
    let (retired_mic_tx, retired_mic_rx) = RingBuffer::new(RETIRED_MIC_QUEUE_CAPACITY);
    let status = Arc::new(StreamStatus::default());

    // The mic consumer is attached only after `play()` succeeds (see below), so a
    // failed open attempt hands it back to the caller instead of dropping it.
    let mut mixer = Mixer::new(
        channels,
        sample_rate,
        initial_global_volume,
        cmd_rx,
        retired_tx,
        Arc::clone(&status),
        None,
    )
    .with_retired_mic(retired_mic_tx);

    // Lock-free fault flag records WASAPI/device invalidation without allocating (`AGENTS.md` §5).
    let fault_status = Arc::clone(&status);
    let err_fn = move |err: cpal::Error| {
        let kind = err.kind();
        if is_fatal_stream_error(kind) {
            fault_status
                .last_error_code
                .store(encode_error_kind(kind), Ordering::Relaxed);
            fault_status.active_slots_mask.store(0, Ordering::Relaxed);
            fault_status.preview_playing.store(false, Ordering::Relaxed);
            fault_status.stream_faulted.store(true, Ordering::Release);
        }
    };

    let build_res = match sample_format {
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _| mixer.render_f32(data),
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _| mixer.render_i16(data),
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _| mixer.render_u16(data),
            err_fn,
            None,
        ),
        _ => unreachable!(),
    };

    let stream = match build_res {
        Ok(s) => s,
        Err(e) => {
            return Err((
                format!("Failed to open output stream: {e}"),
                mic_passthrough,
            ));
        }
    };

    if let Err(e) = stream.play() {
        return Err((
            format!("Failed to start audio stream: {e}"),
            mic_passthrough,
        ));
    }

    let mut endpoint = OutputEndpoint {
        _stream: stream,
        device_id,
        sample_rate,
        cmd_tx,
        retired_rx,
        retired_mic_rx,
        status,
    };
    if mic_passthrough.is_some() {
        endpoint.send(MixerCommand::ReplaceMic(mic_passthrough));
    }

    Ok(endpoint)
}
