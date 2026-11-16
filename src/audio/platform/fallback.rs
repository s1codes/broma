// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Portable fallback audio router for future OS targets (e.g. macOS BlackHole).

use cpal::traits::HostTrait;
use cpal::{Device, Host};

use crate::audio::mic_bridge::MicBridge;
use crate::audio::platform::ActiveVirtualRoute;
use crate::audio::{DEVICE_DISABLED, DEVICE_SYSTEM_DEFAULT_MIC, device_display_name};

pub struct FallbackAudioRouter;

impl Default for FallbackAudioRouter {
    fn default() -> Self {
        Self
    }
}

impl FallbackAudioRouter {
    pub fn new() -> Self {
        Self
    }

    pub fn list_microphones(&self, host: &Host) -> Vec<String> {
        let mut out = vec![DEVICE_SYSTEM_DEFAULT_MIC.to_string()];
        if let Ok(devices) = host.input_devices() {
            for dev in devices {
                let name = device_display_name(&dev);
                if !name.is_empty() && !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out.push(DEVICE_DISABLED.to_string());
        out
    }

    pub fn list_virtual_outputs(&self, host: &Host) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(devices) = host.output_devices() {
            for dev in devices {
                let name = device_display_name(&dev);
                if !name.is_empty() && !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out.push(DEVICE_DISABLED.to_string());
        out
    }

    pub fn default_virtual_output(&self, _available: &[String]) -> String {
        DEVICE_DISABLED.to_string()
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

        let mut outputs = host
            .output_devices()
            .map_err(|e| format!("Failed to enumerate outputs: {e}"))?;
        let sink_device = outputs
            .find(|d| device_display_name(d) == output_selection)
            .ok_or_else(|| format!("Output device '{output_selection}' not available"))?;

        let mic_bridge = if mic_selection.is_empty() || mic_selection == DEVICE_DISABLED {
            None
        } else {
            resolve_input(host, mic_selection).and_then(|dev| MicBridge::open(dev).ok())
        };

        Ok(Some(ActiveVirtualRoute {
            sink_device,
            mic_bridge,
            route_warning: None,
        }))
    }
}

pub fn is_broma_internal_sink_name(_name: &str) -> bool {
    false
}

fn resolve_input(host: &Host, selection: &str) -> Option<Device> {
    if selection == DEVICE_SYSTEM_DEFAULT_MIC {
        return host.default_input_device();
    }
    host.input_devices()
        .ok()?
        .find(|d| device_display_name(d) == selection)
}
