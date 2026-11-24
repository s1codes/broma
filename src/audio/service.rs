// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Background Audio Coordinator Service (`AGENTS.md` §4–5, Milestone 5).
//!
//! Runs `AudioEngine` (`cpal` dual streams, `LruClipCache`, `symphonia` + `rubato`
//! decoding, and OS virtual mic lifecycle) on a dedicated parked OS thread so:
//! - Audio + Virtual Mic + Global Hotkeys operate identically whether the UI
//!   window is open or closed to tray.
//! - First-play decoding/resampling and device switches never stall GUI frames.
//! - While idle (nothing playing), the coordinator blocks on `recv()` at 0% CPU.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::audio::AudioEngine;
use crate::daemon_bus::{DaemonEvent, DaemonSender};
use crate::storage::{self, SlotConfig};

const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(35);

#[derive(Debug, Clone)]
pub struct PlayableSlot {
    pub path: Option<PathBuf>,
    pub custom_volume: Option<u8>,
    pub pitch: f32,
}

enum AudioCommand {
    SyncSlots {
        slots: Vec<PlayableSlot>,
        visible_count: usize,
        global_volume: u8,
    },
    ToggleSlot(usize),
    StopSlot(usize),
    StopAll,
    TogglePreview {
        path: PathBuf,
        volume: u8,
        pitch: f32,
    },
    UpdatePreview {
        volume: u8,
        pitch: f32,
    },
    StopPreview,
    SetGlobalVolume(u8),
    SetMicrophone(String),
    SetVirtualOutput(String),
    SetMonitorOutput(String),
    SetMonitorVolume(u8),
    SetCableVolume(u8),
    ClearCache,
    RefreshDevices,
    InvalidateCachedFile(PathBuf),
    Shutdown,
}

#[derive(Default)]
pub struct SharedAudioState {
    pub active_slots_mask: AtomicU64,
    pub preview_playing: AtomicBool,
    pub window_open: AtomicBool,
}

/// Cheap, cloneable handle shared by the UI, System Tray, and Global Hotkeys.
#[derive(Clone)]
pub struct AudioHandle {
    tx: Sender<AudioCommand>,
    shared: Arc<SharedAudioState>,
}

/// Owned lifecycle guard held by `App` so clean exit can deterministically
/// shut down and join the background audio thread (unloading virtual mics).
pub struct AudioServiceGuard {
    handle: AudioHandle,
    join_handle: Option<JoinHandle<()>>,
}

pub struct AudioServiceBootstrap {
    pub guard: AudioServiceGuard,
    pub microphones: Vec<String>,
    pub virtual_outputs: Vec<String>,
    pub monitor_outputs: Vec<String>,
    pub selected_microphone: String,
    pub selected_virtual_output: String,
    pub selected_monitor: String,
    /// Startup routing failure (e.g. Virtual Cable missing) surfaced in Settings
    /// without emitting a bus event while the window is closed to tray.
    pub initial_audio_error: Option<String>,
}

impl AudioServiceGuard {
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        saved_microphone: Option<&str>,
        saved_virtual_output: Option<&str>,
        saved_monitor_output: Option<&str>,
        global_volume: u8,
        monitor_volume: u8,
        cable_volume: u8,
        window_initially_open: bool,
        bus: DaemonSender,
    ) -> AudioServiceBootstrap {
        let (cmd_tx, cmd_rx) = mpsc::channel::<AudioCommand>();
        let (init_tx, init_rx) = mpsc::sync_channel(1);

        let shared = Arc::new(SharedAudioState {
            active_slots_mask: AtomicU64::new(0),
            preview_playing: AtomicBool::new(false),
            window_open: AtomicBool::new(window_initially_open),
        });
        let thread_shared = Arc::clone(&shared);
        let saved_mic = saved_microphone.map(str::to_string);
        let saved_out = saved_virtual_output.map(str::to_string);
        let saved_mon = saved_monitor_output.map(str::to_string);

        let join_handle = thread::Builder::new()
            .name("broma-audio-service".to_string())
            .spawn(move || {
                let mut boot = AudioEngine::bootstrap(
                    saved_mic.as_deref(),
                    saved_out.as_deref(),
                    saved_mon.as_deref(),
                    global_volume,
                    monitor_volume,
                    cable_volume,
                );
                let initial_audio_error = boot.engine.take_pending_device_error();
                let _ = init_tx.send((
                    boot.microphones,
                    boot.virtual_outputs,
                    boot.monitor_outputs,
                    boot.selected_microphone,
                    boot.selected_virtual_output,
                    boot.selected_monitor,
                    initial_audio_error,
                ));
                run_audio_coordinator(boot.engine, global_volume, cmd_rx, thread_shared, bus);
            })
            .expect("spawn broma-audio-service thread");

        let (
            microphones,
            virtual_outputs,
            monitor_outputs,
            selected_microphone,
            selected_virtual_output,
            selected_monitor,
            initial_audio_error,
        ) = init_rx.recv().unwrap_or_else(|_| {
            (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
                String::new(),
                String::new(),
                None,
            )
        });

        let handle = AudioHandle { tx: cmd_tx, shared };
        AudioServiceBootstrap {
            guard: Self {
                handle,
                join_handle: Some(join_handle),
            },
            microphones,
            virtual_outputs,
            monitor_outputs,
            selected_microphone,
            selected_virtual_output,
            selected_monitor,
            initial_audio_error,
        }
    }

    pub fn handle(&self) -> AudioHandle {
        self.handle.clone()
    }

    /// Deterministically stops streams, unloads OS virtual mic modules, and joins
    /// the background audio thread before process exit.
    pub fn shutdown(&mut self) {
        let _ = self.handle.tx.send(AudioCommand::Shutdown);
        if let Some(jh) = self.join_handle.take() {
            let _ = jh.join();
        }
    }
}

impl Drop for AudioServiceGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl AudioHandle {
    pub fn set_window_open(&self, open: bool) {
        self.shared.window_open.store(open, Ordering::Relaxed);
    }

    pub fn active_slots_mask(&self) -> u64 {
        self.shared.active_slots_mask.load(Ordering::Relaxed)
    }

    pub fn is_previewing(&self) -> bool {
        self.shared.preview_playing.load(Ordering::Relaxed)
    }

    pub fn sync_slots(&self, slots: &[SlotConfig], visible_count: usize, global_volume: u8) {
        let playable: Vec<PlayableSlot> = slots
            .iter()
            .map(|slot| PlayableSlot {
                path: slot
                    .stored_file
                    .as_deref()
                    .and_then(storage::resolve_stored_sound),
                custom_volume: slot.custom_volume,
                pitch: slot.pitch_multiplier(),
            })
            .collect();

        let _ = self.tx.send(AudioCommand::SyncSlots {
            slots: playable,
            visible_count,
            global_volume,
        });
    }

    pub fn toggle_slot(&self, slot_index: usize) {
        let _ = self.tx.send(AudioCommand::ToggleSlot(slot_index));
    }

    pub fn stop_slot(&self, slot_index: usize) {
        let _ = self.tx.send(AudioCommand::StopSlot(slot_index));
    }

    pub fn stop_all(&self) {
        let _ = self.tx.send(AudioCommand::StopAll);
    }

    pub fn toggle_preview(&self, path: PathBuf, volume: u8, pitch: f32) {
        let _ = self.tx.send(AudioCommand::TogglePreview {
            path,
            volume,
            pitch,
        });
    }

    pub fn update_preview(&self, volume: u8, pitch: f32) {
        let _ = self.tx.send(AudioCommand::UpdatePreview { volume, pitch });
    }

    pub fn stop_preview(&self) {
        let _ = self.tx.send(AudioCommand::StopPreview);
    }

    pub fn set_global_volume(&self, volume: u8) {
        let _ = self.tx.send(AudioCommand::SetGlobalVolume(volume));
    }

    pub fn set_microphone(&self, selection: &str) {
        let _ = self
            .tx
            .send(AudioCommand::SetMicrophone(selection.to_string()));
    }

    pub fn set_virtual_output(&self, selection: &str) {
        let _ = self
            .tx
            .send(AudioCommand::SetVirtualOutput(selection.to_string()));
    }

    pub fn set_monitor_output(&self, selection: &str) {
        let _ = self
            .tx
            .send(AudioCommand::SetMonitorOutput(selection.to_string()));
    }

    pub fn set_monitor_volume(&self, volume: u8) {
        let _ = self.tx.send(AudioCommand::SetMonitorVolume(volume));
    }

    pub fn set_cable_volume(&self, volume: u8) {
        let _ = self.tx.send(AudioCommand::SetCableVolume(volume));
    }

    pub fn clear_cache(&self) {
        let _ = self.tx.send(AudioCommand::ClearCache);
    }

    pub fn refresh_devices(&self) {
        let _ = self.tx.send(AudioCommand::RefreshDevices);
    }

    pub fn invalidate_cached_file(&self, path: PathBuf) {
        let _ = self.tx.send(AudioCommand::InvalidateCachedFile(path));
    }
}

fn run_audio_coordinator(
    mut engine: AudioEngine,
    initial_global_volume: u8,
    cmd_rx: Receiver<AudioCommand>,
    shared: Arc<SharedAudioState>,
    bus: DaemonSender,
) {
    let mut playable_slots: Vec<PlayableSlot> = Vec::new();
    let mut visible_count: usize = 0;
    let mut global_volume: u8 = initial_global_volume;

    loop {
        let mask_before = engine.active_slots_mask();
        let preview_before = engine.is_previewing();
        publish_and_notify(&shared, &bus, mask_before, preview_before);

        // Block indefinitely at 0% CPU whenever no voice or preview is active.
        let next_cmd = if mask_before == 0 && !preview_before {
            match cmd_rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => break,
            }
        } else {
            match cmd_rx.recv_timeout(ACTIVE_POLL_INTERVAL) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => {
                    engine.reclaim_retired_buffers();
                    forward_device_error(&mut engine, &shared, &bus);
                    None
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };

        let Some(cmd) = next_cmd else {
            continue;
        };

        if matches!(cmd, AudioCommand::Shutdown) {
            engine.stop_all();
            publish_and_notify(&shared, &bus, 0, false);
            break;
        }

        handle_audio_command(
            cmd,
            &mut engine,
            &mut playable_slots,
            &mut visible_count,
            &mut global_volume,
            &shared,
            &bus,
        );
    }
}

fn handle_audio_command(
    cmd: AudioCommand,
    engine: &mut AudioEngine,
    playable_slots: &mut Vec<PlayableSlot>,
    visible_count: &mut usize,
    global_volume: &mut u8,
    shared: &SharedAudioState,
    bus: &DaemonSender,
) {
    match cmd {
        AudioCommand::SyncSlots {
            slots,
            visible_count: vis,
            global_volume: gvol,
        } => {
            *playable_slots = slots;
            *visible_count = vis;
            *global_volume = gvol;
        }
        AudioCommand::ToggleSlot(idx) => {
            if idx >= *visible_count {
                return;
            }
            let Some(slot) = playable_slots.get(idx) else {
                return;
            };
            let Some(path) = slot.path.as_deref() else {
                return;
            };
            if let Err(err) =
                engine.toggle_slot(idx, path, slot.custom_volume, *global_volume, slot.pitch)
                && shared.window_open.load(Ordering::Relaxed)
            {
                bus.send(DaemonEvent::AudioError(err));
            }
            forward_device_error(engine, shared, bus);
        }
        AudioCommand::StopSlot(idx) => {
            engine.stop(idx);
        }
        AudioCommand::StopAll => {
            engine.stop_all();
        }
        AudioCommand::TogglePreview {
            path,
            volume,
            pitch,
        } => {
            if let Err(err) = engine.toggle_preview(&path, volume, pitch)
                && shared.window_open.load(Ordering::Relaxed)
            {
                bus.send(DaemonEvent::AudioError(err));
            }
            forward_device_error(engine, shared, bus);
        }
        AudioCommand::UpdatePreview { volume, pitch } => {
            engine.update_preview(volume, pitch);
        }
        AudioCommand::StopPreview => {
            engine.stop_preview();
        }
        AudioCommand::SetGlobalVolume(vol) => {
            *global_volume = vol;
            engine.set_global_volume(vol);
        }
        AudioCommand::SetMicrophone(mic) => {
            engine.set_microphone(&mic);
            if !forward_device_error(engine, shared, bus) {
                notify_status_cleared(engine, shared, bus);
            }
        }
        AudioCommand::SetVirtualOutput(out) => {
            engine.set_virtual_output(&out);
            if !forward_device_error(engine, shared, bus) {
                notify_status_cleared(engine, shared, bus);
            }
        }
        AudioCommand::SetMonitorOutput(out) => {
            engine.set_monitor_output(&out);
            if !forward_device_error(engine, shared, bus) {
                notify_status_cleared(engine, shared, bus);
            }
        }
        AudioCommand::SetMonitorVolume(vol) => {
            engine.set_monitor_volume(vol);
        }
        AudioCommand::SetCableVolume(vol) => {
            engine.set_cable_volume(vol);
        }
        AudioCommand::ClearCache => {
            engine.clear_cache();
        }
        AudioCommand::RefreshDevices => {
            // Revive any unplugged/faulted route before reporting the new lists.
            engine.recover_missing_or_faulted();
            let (microphones, virtual_outputs, monitor_outputs) = engine.refresh_device_lists();
            bus.send(DaemonEvent::DevicesRefreshed {
                microphones,
                virtual_outputs,
                monitor_outputs,
            });
            if !forward_device_error(engine, shared, bus) {
                notify_status_cleared(engine, shared, bus);
            }
        }
        AudioCommand::InvalidateCachedFile(path) => {
            engine.invalidate_cached_file(&path);
        }
        AudioCommand::Shutdown => {}
    }
}

/// Surfaces a pending routing error to the UI while the window is open.
/// Returns `true` when an error was sent.
fn forward_device_error(
    engine: &mut AudioEngine,
    shared: &SharedAudioState,
    bus: &DaemonSender,
) -> bool {
    if !shared.window_open.load(Ordering::Relaxed) {
        return false;
    }
    match engine.take_pending_device_error() {
        Some(err) => {
            bus.send(DaemonEvent::AudioError(err));
            true
        }
        None => false,
    }
}

/// Tells the UI to clear a stale audio error after a clean device switch/refresh.
fn notify_status_cleared(engine: &mut AudioEngine, shared: &SharedAudioState, bus: &DaemonSender) {
    if !shared.window_open.load(Ordering::Relaxed) || engine.last_device_error.is_some() {
        return;
    }
    engine.clear_device_error();
    bus.send(DaemonEvent::AudioStatusCleared);
}

fn publish_and_notify(
    shared: &SharedAudioState,
    bus: &DaemonSender,
    new_mask: u64,
    new_preview: bool,
) {
    let prev_mask = shared.active_slots_mask.swap(new_mask, Ordering::Relaxed);
    let prev_preview = shared.preview_playing.swap(new_preview, Ordering::Relaxed);

    if (prev_mask != new_mask || prev_preview != new_preview)
        && shared.window_open.load(Ordering::Relaxed)
    {
        bus.send(DaemonEvent::AudioPlaybackChanged);
    }
}
