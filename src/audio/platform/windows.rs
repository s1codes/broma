// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Windows-specific audio routing (WASAPI + VB-Cable / VoiceMeeter).
//!
//! Kept completely separate from Linux PipeWire/PulseAudio code so any
//! Windows-specific device discovery, WASAPI stream, or VB-Cable passthrough
//! issues can be debugged in isolation.
//!
//! Behavior on Windows:
//! - Discovers WASAPI output endpoints and sorts virtual audio cables
//!   (`CABLE Input`, `VoiceMeeter`, `Virtual Audio Cable`) to the top of the
//!   Output list, auto-selecting one on first launch if installed.
//! - Filters virtual loopback recording endpoints (`CABLE Output`, `Stereo Mix`,
//!   `VoiceMeeter Out`) out of the Microphone picker to prevent feedback loops.
//! - Routes the user's selected hardware microphone through a lock-free `rtrb`
//!   capture bridge (`MicBridge`) directly into the Virtual Cable output mixer.
//! - Never fails silently: if a Windows microphone is unplugged or blocked by
//!   Windows Microphone Privacy settings, keeps soundboard playback alive on
//!   the virtual cable and reports an explicit `route_warning`.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, Host};

use crate::audio::mic_bridge::{MicBridge, MicPassthroughConsumer};
use crate::audio::platform::ActiveVirtualRoute;
use crate::audio::{DEVICE_DISABLED, DEVICE_SYSTEM_DEFAULT_MIC, device_display_name};

pub struct WindowsAudioRouter;

impl Default for WindowsAudioRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsAudioRouter {
    pub fn new() -> Self {
        Self
    }

    pub fn list_microphones(&self, host: &Host) -> Vec<String> {
        let raw = host
            .input_devices()
            .ok()
            .into_iter()
            .flatten()
            .map(|dev| device_display_name(&dev));
        filter_windows_microphones(raw)
    }

    pub fn list_virtual_outputs(&self, host: &Host) -> Vec<String> {
        let raw = host
            .output_devices()
            .ok()
            .into_iter()
            .flatten()
            .map(|dev| device_display_name(&dev));
        sort_and_dedupe_windows_outputs(raw)
    }

    pub fn default_virtual_output(&self, available: &[String]) -> String {
        // Auto-select VB-Cable / VoiceMeeter on first launch if installed; otherwise Disabled.
        available
            .iter()
            .find(|name| is_windows_virtual_cable_output(name))
            .cloned()
            .unwrap_or_else(|| DEVICE_DISABLED.to_string())
    }

    pub fn prepare_route(
        &mut self,
        host: &Host,
        mic_selection: &str,
        output_selection: &str,
    ) -> Result<Option<ActiveVirtualRoute>, String> {
        if output_selection.is_empty() || output_selection == DEVICE_DISABLED {
            return Ok(None);
        }

        let sink_device = find_windows_output_device(host, output_selection).ok_or_else(|| {
            format!("Windows output device '{output_selection}' is not available")
        })?;

        let (mic_bridge, route_warning) = self.open_mic_bridge(host, mic_selection);

        Ok(Some(ActiveVirtualRoute {
            sink_device,
            mic_bridge,
            route_warning,
        }))
    }

    /// Opens the hardware microphone capture bridge for `mic_selection`.
    ///
    /// Shared by initial routing and by isolated mic recovery, so a failed
    /// attempt always reports the same explicit warning instead of silence.
    pub fn open_mic_bridge(
        &self,
        host: &Host,
        mic_selection: &str,
    ) -> (Option<(MicBridge, MicPassthroughConsumer)>, Option<String>) {
        if mic_selection.is_empty() || mic_selection == DEVICE_DISABLED {
            return (None, None);
        }

        match find_windows_input_device(host, mic_selection) {
            Some(mic_dev) => match MicBridge::open(mic_dev) {
                Ok(bridge) => (Some(bridge), None),
                Err(err) => (
                    None,
                    Some(format!(
                        "Microphone passthrough unavailable ({err}) — check Windows Microphone Privacy settings"
                    )),
                ),
            },
            None => (
                None,
                Some(format!(
                    "Microphone '{mic_selection}' not found — soundboard active without mic passthrough"
                )),
            ),
        }
    }
}

/// Returns the index of the enumerated endpoint representing the OS default device.
///
/// Matching prefers the backend `DeviceId` (e.g. `"wasapi:{0.0.1.00000000}.{...}"`)
/// and falls back to the display name. Pure so it is unit-testable off Windows.
pub(crate) fn matching_index<'a, I>(
    default_id: Option<&str>,
    default_name: &str,
    candidates: I,
) -> Option<usize>
where
    I: IntoIterator<Item = (Option<&'a str>, &'a str)>,
{
    let candidates: Vec<(Option<&str>, &str)> = candidates.into_iter().collect();

    if let Some(id) = default_id
        && let Some(idx) = candidates.iter().position(|(cid, _)| *cid == Some(id))
    {
        return Some(idx);
    }

    if !default_name.is_empty() {
        return candidates
            .iter()
            .position(|(_, name)| *name == default_name);
    }

    None
}

/// Re-resolves the OS default `Device` handle to its concrete enumerated peer.
///
/// `cpal 0.18.2` WASAPI (#1402) can hand back a default endpoint whose COM
/// context is poisoned after stream churn; reopening the matching enumerated
/// handle avoids reusing that default endpoint directly. Falls back to
/// `default_dev` when enumeration fails or no peer matches.
pub(crate) fn resolve_specific_device(
    default_dev: Device,
    enumerated: Option<impl Iterator<Item = Device>>,
) -> Device {
    let Some(devices) = enumerated else {
        return default_dev;
    };
    let devices: Vec<Device> = devices.collect();

    let default_id = default_dev.id().ok().map(|id| id.to_string());
    let default_name = device_display_name(&default_dev);

    let ids: Vec<Option<String>> = devices
        .iter()
        .map(|dev| dev.id().ok().map(|id| id.to_string()))
        .collect();
    let names: Vec<String> = devices.iter().map(device_display_name).collect();
    let entries = ids
        .iter()
        .map(Option::as_deref)
        .zip(names.iter().map(String::as_str));

    match matching_index(default_id.as_deref(), &default_name, entries) {
        Some(idx) => devices.into_iter().nth(idx).unwrap_or(default_dev),
        None => default_dev,
    }
}

pub fn is_broma_internal_sink_name(_name: &str) -> bool {
    false
}

pub(crate) fn filter_windows_microphones<I>(raw_names: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut out = vec![DEVICE_SYSTEM_DEFAULT_MIC.to_string()];
    for raw in raw_names {
        let name = raw.trim();
        if !name.is_empty()
            && !is_windows_loopback_input(name)
            && name != DEVICE_SYSTEM_DEFAULT_MIC
            && name != DEVICE_DISABLED
            && !out.iter().any(|existing| existing == name)
        {
            out.push(name.to_string());
        }
    }
    out.push(DEVICE_DISABLED.to_string());
    out
}

pub(crate) fn sort_and_dedupe_windows_outputs<I>(raw_names: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut virtual_cables = Vec::new();
    let mut regular_outputs = Vec::new();

    for raw in raw_names {
        let name = raw.trim();
        if name.is_empty() || name == DEVICE_DISABLED {
            continue;
        }
        if is_windows_virtual_cable_output(name) {
            if !virtual_cables.iter().any(|existing| existing == name) {
                virtual_cables.push(name.to_string());
            }
        } else if !regular_outputs.iter().any(|existing| existing == name) {
            regular_outputs.push(name.to_string());
        }
    }

    let mut out = Vec::with_capacity(virtual_cables.len() + regular_outputs.len() + 1);
    out.extend(virtual_cables);
    out.extend(regular_outputs);
    out.push(DEVICE_DISABLED.to_string());
    out
}

pub(crate) fn is_windows_virtual_cable_output(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cable input")
        || lower.contains("vb-audio")
        || lower.contains("voicemeeter")
        || lower.contains("virtual audio cable")
        || lower.contains("virtual cable")
}

pub(crate) fn is_windows_loopback_input(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cable output")
        || lower.contains("vb-audio")
        || lower.contains("voicemeeter out")
        || lower.contains("stereo mix")
        || lower.contains("what u hear")
}

fn find_windows_output_device(host: &Host, selection: &str) -> Option<Device> {
    let mut devices = host.output_devices().ok()?;
    devices.find(|dev| device_display_name(dev) == selection)
}

fn find_windows_input_device(host: &Host, selection: &str) -> Option<Device> {
    if selection == DEVICE_SYSTEM_DEFAULT_MIC {
        if let Some(default_mic) = host.default_input_device() {
            // Prefer the concrete enumerated handle (#1402) when one matches.
            let default_mic = resolve_specific_device(default_mic, host.input_devices().ok());
            let name = device_display_name(&default_mic);
            if !is_windows_loopback_input(&name) {
                return Some(default_mic);
            }
        }
        // If Windows default recording device was set to CABLE Output, pick first real mic.
        let mut devices = host.input_devices().ok()?;
        return devices.find(|dev| {
            let name = device_display_name(dev);
            !name.is_empty() && !is_windows_loopback_input(&name)
        });
    }

    let mut devices = host.input_devices().ok()?;
    devices.find(|dev| device_display_name(dev) == selection)
}
