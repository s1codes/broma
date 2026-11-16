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

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, DeviceId, SampleFormat, Stream, StreamConfig, SupportedBufferSize,
    SupportedStreamConfig,
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
/// Target callback size (~10.6 ms at 48 kHz): imperceptible trigger latency without crackling.
const PREFERRED_BUFFER_FRAMES: u32 = 512;

pub(crate) struct OutputEndpoint {
    _stream: Stream,
    device_id: Option<DeviceId>,
    sample_rate: u32,
    cmd_tx: Producer<MixerCommand>,
    retired_rx: Consumer<Arc<[i16]>>,
    pub(crate) status: Arc<StreamStatus>,
}

impl OutputEndpoint {
    fn reclaim_retired(&mut self) {
        while let Ok(buf) = self.retired_rx.pop() {
            drop(buf);
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
        self.reopen_streams();
    }

    pub fn set_virtual_output(&mut self, selection: &str) {
        if self.virtual_output_selection == selection {
            return;
        }
        self.stop_all();
        self.virtual_output_selection = selection.to_string();
        self.reopen_streams();
    }

    pub fn set_monitor_output(&mut self, selection: &str) {
        if self.monitor_selection == selection {
            return;
        }
        self.stop_all();
        self.monitor_selection = selection.to_string();
        self.reopen_streams();
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
        let monitor_faulted = self
            .monitor
            .as_ref()
            .is_some_and(|ep| ep.status.stream_faulted.load(Ordering::Relaxed));
        let cable_faulted = self
            .virtual_cable
            .as_ref()
            .is_some_and(|ep| ep.status.stream_faulted.load(Ordering::Relaxed));
        let mic_faulted = self.mic_bridge.as_ref().is_some_and(MicBridge::is_faulted);

        if monitor_faulted || cable_faulted || mic_faulted {
            self.reopen_streams();
            true
        } else {
            false
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

        let follows_global = custom_volume.is_none();
        let base_gain = volume_to_linear(custom_volume.unwrap_or(global_volume_percent));
        let safe_pitch = pitch.clamp(0.25, 4.0);

        let cmd = MixerCommand::Play {
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
            m.send(cmd.clone());
            if slot_index < 64 {
                m.status
                    .active_slots_mask
                    .fetch_or(1u64 << slot_index, Ordering::Relaxed);
            }
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(MixerCommand::SetGlobalVolume(self.global_volume));
            c.send(cmd);
            if slot_index < 64 {
                c.status
                    .active_slots_mask
                    .fetch_or(1u64 << slot_index, Ordering::Relaxed);
            }
        }

        Ok(adjusted_duration_ms(clip.duration_ms(), safe_pitch))
    }

    pub fn stop(&mut self, slot_index: usize) {
        let cmd = MixerCommand::StopTag(VoiceTag::Slot(slot_index));
        if let Some(m) = &mut self.monitor {
            m.send(cmd.clone());
            if slot_index < 64 {
                m.status
                    .active_slots_mask
                    .fetch_and(!(1u64 << slot_index), Ordering::Relaxed);
            }
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(cmd);
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
            m.status.preview_playing.store(true, Ordering::Relaxed);
        } else if let Some(c) = &mut self.virtual_cable {
            // Only fall back if no local headphone output exists on the system.
            c.send(cmd);
            c.status.preview_playing.store(true, Ordering::Relaxed);
        }

        Ok(Some(adjusted_duration_ms(clip.duration_ms(), safe_pitch)))
    }

    /// Live-updates volume and pitch while a modal preview clip is playing.
    pub fn update_preview(&mut self, volume_percent: u8, pitch: f32) {
        let cmd = MixerCommand::UpdatePreview {
            gain: volume_to_linear(volume_percent),
            pitch: pitch.clamp(0.25, 4.0),
        };
        if let Some(m) = &mut self.monitor {
            m.send(cmd.clone());
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(cmd);
        }
    }

    pub fn stop_preview(&mut self) {
        let cmd = MixerCommand::StopTag(VoiceTag::Preview);
        if let Some(m) = &mut self.monitor {
            m.send(cmd.clone());
            m.status.preview_playing.store(false, Ordering::Relaxed);
        }
        if let Some(c) = &mut self.virtual_cable {
            c.send(cmd);
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

        let host = cpal::default_host();

        // 1. Prepare Stream 2: Voice Chat / Virtual Mic output (+ default Mic Passthrough).
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

        // 2. Open Stream 1: Local Headphones/Speakers Monitor,
        //    ensuring we never bind Monitor to the Virtual Cable / internal bus itself.
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

    let mut best = None;
    for range in ranges {
        if !matches!(
            range.sample_format(),
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
        ) {
            continue;
        }
        let rate = 48_000.clamp(range.min_sample_rate(), range.max_sample_rate());
        let candidate = range.with_sample_rate(rate);
        if candidate.channels() == 2 {
            return Ok(candidate);
        }
        if best.is_none() {
            best = Some(candidate);
        }
    }

    best.ok_or_else(|| "No supported PCM format on output device".to_string())
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
    let status = Arc::new(StreamStatus::default());

    let mut mixer = Mixer::new(
        channels,
        sample_rate,
        initial_global_volume,
        cmd_rx,
        retired_tx,
        Arc::clone(&status),
        mic_passthrough,
    );

    // Lock-free fault flag records WASAPI/device invalidation without allocating (`AGENTS.md` §5).
    let fault_status = Arc::clone(&status);
    let err_fn = move |_err| {
        fault_status.stream_faulted.store(true, Ordering::Relaxed);
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
        Err(e) => return Err((format!("Failed to open output stream: {e}"), None)),
    };

    if let Err(e) = stream.play() {
        return Err((format!("Failed to start audio stream: {e}"), None));
    }

    Ok(OutputEndpoint {
        _stream: stream,
        device_id,
        sample_rate,
        cmd_tx,
        retired_rx,
        status,
    })
}
