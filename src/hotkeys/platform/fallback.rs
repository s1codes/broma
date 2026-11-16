// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Portable hotkey stub for future OS targets (e.g. macOS).

use crate::audio::service::AudioHandle;
use crate::daemon_bus::DaemonSender;
use crate::storage::SlotConfig;

pub struct FallbackHotkeyEngine;

impl FallbackHotkeyEngine {
    pub fn new(_audio: AudioHandle, _bus: DaemonSender) -> Self {
        Self
    }

    pub fn set_suspended(&mut self, _suspended: bool) {}

    pub fn sync_slots(&mut self, _slots: &[SlotConfig], _visible_count: usize) {}

    pub fn shutdown(&mut self) {}
}
