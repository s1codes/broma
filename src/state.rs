// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Application state and event-driven daemon update logic.
//!
//! Pre-computes display labels inside `update()` so `view()` borrows `&str`
//! slices with zero `format!()` allocations on hover/scroll redraws.
//! Audio, Global Hotkeys, and the System Tray communicate through the unified
//! [`crate::daemon_bus`] and [`crate::audio::service::AudioHandle`] so playback
//! works identically with the window open or closed to tray.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, key::Named};
use iced::{Element, Subscription, Task, window};

use crate::audio::service::{AudioHandle, AudioServiceGuard};
use crate::daemon_bus::{self, DaemonEvent, DaemonReceiver};
use crate::hotkeys::{self, PlatformHotkeyEngine};
use crate::storage::{
    self, AppConfig, DEFAULT_PITCH_PERCENT, GRID_COLS, MAX_GRID_ROWS, MIN_GRID_ROWS, SlotConfig,
};
use crate::tray::TrayController;
use crate::ui;
use crate::ui::components::pad_grid::PadViewItem;

#[derive(Debug, Clone)]
pub enum Message {
    WindowCloseRequested(window::Id),
    Daemon(DaemonEvent),

    PadClicked(usize),
    OpenSlotModal(usize),

    ModalBackdropClicked,
    ModalNameChanged(String),
    ModalPickAudioFile,
    ModalAudioFilePicked(Option<PathBuf>),
    ModalVolumeChanged(u8),
    ModalUseGlobalVolume,
    ModalPitchChanged(u16),
    ModalResetPitch,
    ModalTogglePreview,
    ModalToggleRecordKeybind,
    ModalClearKeybind,
    ModalClearSlot,
    ModalCancel,
    ModalSave,

    ModalKeepEditing,
    ModalDiscardChanges,

    KeyCaptured(keyboard::Event),

    SelectMicrophone(String),
    SelectOutput(String),
    ShrinkGrid,
    GrowGrid,
    ToggleCloseToTray(bool),
    GlobalVolumeChanged(u8),
    CommitGlobalVolume,
    StopAll,

    OpenSettings,
    SettingsBackdropClicked,
    SettingsClose,
    SettingsRefreshDevices,
    SettingsSelectMonitor(String),
    SettingsMonitorVolumeChanged(u8),
    SettingsCableVolumeChanged(u8),
    SettingsCommitStreamVolumes,
    SettingsExportBackup,
    SettingsExportBackupPicked(Option<PathBuf>),
    SettingsImportBackup,
    SettingsImportBackupPicked(Option<PathBuf>),
    SettingsRequestDeleteAll,
    SettingsCancelDeleteAll,
    SettingsConfirmDeleteAll,
}

pub struct ModalState {
    pub slot_index: usize,
    pub original: SlotConfig,
    pub draft: SlotConfig,
    pub pending_source_path: Option<PathBuf>,
    pub is_recording_keybind: bool,
    pub show_unsaved_prompt: bool,
    pub error_hint: Option<String>,
    pub conflicting_slot: Option<usize>,

    // Pre-formatted strings so `view()` never allocates
    pub title_label: String,
    pub file_label: String,
    pub volume_label: String,
    pub pitch_label: String,
    pub preview_status_label: String,
    pub keybind_label: String,
}

impl ModalState {
    fn new(slot_index: usize, current: SlotConfig, global_volume: u8) -> Self {
        let mut modal = Self {
            slot_index,
            original: current.clone(),
            draft: current,
            pending_source_path: None,
            is_recording_keybind: false,
            show_unsaved_prompt: false,
            error_hint: None,
            conflicting_slot: None,
            title_label: format!("Configure Button #{}", slot_index + 1),
            file_label: String::new(),
            volume_label: String::new(),
            pitch_label: String::new(),
            preview_status_label: String::new(),
            keybind_label: String::new(),
        };
        modal.refresh_labels(global_volume, false);
        modal
    }

    pub fn is_dirty(&self) -> bool {
        self.draft != self.original || self.pending_source_path.is_some()
    }

    pub fn has_file(&self) -> bool {
        self.pending_source_path.is_some() || self.draft.stored_file.is_some()
    }

    fn resolve_audio_path(&self) -> Option<PathBuf> {
        if let Some(src) = &self.pending_source_path {
            return Some(src.clone());
        }
        self.draft
            .stored_file
            .as_deref()
            .and_then(storage::resolve_stored_sound)
    }

    fn refresh_labels(&mut self, global_volume: u8, is_previewing: bool) {
        self.file_label = match &self.draft.display_file {
            Some(name) => name.clone(),
            None => "No audio file selected".to_string(),
        };

        self.volume_label = match self.draft.custom_volume {
            Some(vol) => format!("Custom: {vol}%"),
            None => format!("Global ({global_volume}%)"),
        };

        self.pitch_label = format!("{:.2}x", self.draft.pitch_multiplier());

        let eff_vol = self.draft.effective_volume(global_volume);
        let eff_pitch = self.draft.pitch_multiplier();

        self.preview_status_label = if let Some(err) = &self.error_hint {
            err.clone()
        } else if !self.has_file() {
            "Choose an audio file to enable preview".to_string()
        } else if is_previewing {
            format!("Previewing ({eff_vol}% vol, {eff_pitch:.2}x pitch)")
        } else {
            format!("Ready ({eff_vol}% vol, {eff_pitch:.2}x pitch)")
        };

        self.keybind_label = if self.is_recording_keybind {
            "Press any key combo... (Esc to cancel)".to_string()
        } else if let Some(kb) = &self.draft.keybind {
            if let Some(other_idx) = self.conflicting_slot {
                format!("{kb}  (moves from Button #{} on Save)", other_idx + 1)
            } else {
                kb.clone()
            }
        } else {
            "None".to_string()
        };
    }
}

pub struct SettingsModalState {
    pub show_delete_confirm: bool,
    pub status_message: Option<String>,
    pub status_is_error: bool,

    // Pre-formatted strings so `view()` never allocates
    pub monitor_volume_label: String,
    pub cable_volume_label: String,
    pub library_stats_label: String,
    pub status_label: String,
}

impl SettingsModalState {
    fn new(monitor_volume: u8, cable_volume: u8) -> Self {
        let mut state = Self {
            show_delete_confirm: false,
            status_message: None,
            status_is_error: false,
            monitor_volume_label: String::new(),
            cable_volume_label: String::new(),
            library_stats_label: String::new(),
            status_label: String::new(),
        };
        state.refresh_stats();
        state.refresh_labels(monitor_volume, cable_volume);
        state
    }

    fn refresh_stats(&mut self) {
        let stats = storage::compute_library_stats();
        let noun = if stats.sound_files == 1 {
            "sound"
        } else {
            "sounds"
        };
        self.library_stats_label = format!(
            "{} {noun} · {} used",
            stats.sound_files,
            storage::format_bytes(stats.total_bytes)
        );
    }

    fn refresh_labels(&mut self, monitor_volume: u8, cable_volume: u8) {
        self.monitor_volume_label = format!("{monitor_volume}%");
        self.cable_volume_label = format!("{cable_volume}%");
        self.status_label = self.status_message.clone().unwrap_or_default();
    }
}

pub struct App {
    pub config: AppConfig,
    pub audio: AudioHandle,
    audio_guard: AudioServiceGuard,
    hotkeys: PlatformHotkeyEngine,
    tray: TrayController,
    bus_rx: DaemonReceiver,
    main_window: Option<window::Id>,
    last_window_opened_at: Option<Instant>,
    last_hotkey_error: Option<String>,

    pub microphones: Vec<String>,
    pub outputs: Vec<String>,
    pub monitor_outputs: Vec<String>,
    pub modal: Option<ModalState>,
    pub settings_modal: Option<SettingsModalState>,

    // Pre-formatted strings for zero-allocation `view()`
    pub pad_views: Vec<PadViewItem>,
    pub grid_label: String,
    pub global_volume_label: String,
}

impl App {
    pub fn boot() -> (Self, Task<Message>) {
        let mut config = storage::load_config();
        let (bus_tx, bus_rx) = daemon_bus::create();
        #[cfg(target_os = "windows")]
        crate::single_instance::start_listener(bus_tx.clone());

        let audio_boot = AudioServiceGuard::spawn(
            config.microphone.as_deref(),
            config.output_device.as_deref(),
            config.monitor_device.as_deref(),
            config.global_volume,
            config.monitor_volume,
            config.cable_volume,
            true,
            bus_tx.clone(),
        );
        config.microphone = Some(audio_boot.selected_microphone);
        config.output_device = Some(audio_boot.selected_virtual_output);
        config.monitor_device = Some(audio_boot.selected_monitor);

        let audio_handle = audio_boot.guard.handle();
        let hotkeys = PlatformHotkeyEngine::new(audio_handle.clone(), bus_tx.clone());
        let tray = TrayController::new(config.close_to_tray, audio_handle.clone(), bus_tx);

        let (win_id, open_task) = window::open(crate::main_window_settings());

        let mut app = Self {
            config,
            audio: audio_handle,
            audio_guard: audio_boot.guard,
            hotkeys,
            tray,
            bus_rx,
            main_window: Some(win_id),
            last_window_opened_at: Some(Instant::now()),
            last_hotkey_error: None,
            microphones: audio_boot.microphones,
            outputs: audio_boot.virtual_outputs,
            monitor_outputs: audio_boot.monitor_outputs,
            modal: None,
            settings_modal: None,
            pad_views: Vec::new(),
            grid_label: String::new(),
            global_volume_label: String::new(),
        };
        app.rebuild_all_labels();
        app.sync_runtime_slots();

        (app, open_task.discard())
    }

    /// Event-driven subscriptions (`AGENTS.md` §6, §9, §10):
    /// - Single unified `DaemonBus` stream (parked passively at 0% idle CPU).
    /// - Window close interceptor active only while a window is open.
    /// - Keyboard capture listener active only while recording a keybind in the modal.
    pub fn subscription(&self) -> Subscription<Message> {
        let bus_sub = daemon_bus::subscription(&self.bus_rx).map(Message::Daemon);

        if self.main_window.is_none() {
            return bus_sub;
        }

        let close_sub = window::close_requests().map(Message::WindowCloseRequested);

        let recording_sub = if self
            .modal
            .as_ref()
            .is_some_and(|m| m.is_recording_keybind && !m.show_unsaved_prompt)
        {
            keyboard::listen().map(Message::KeyCaptured)
        } else {
            Subscription::none()
        };

        Subscription::batch([bus_sub, close_sub, recording_sub])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WindowCloseRequested(id) => {
                if self.tray.should_close_to_tray() {
                    return self.hide_window_to_tray(id);
                }
                return self.shutdown_and_exit();
            }
            Message::Daemon(evt) => match evt {
                #[cfg(target_os = "windows")]
                DaemonEvent::ExternalLaunch => {
                    if let Some(id) = self.main_window {
                        return window::gain_focus(id);
                    }
                    let (id, open_task) = window::open(crate::main_window_settings());
                    self.main_window = Some(id);
                    self.last_window_opened_at = Some(Instant::now());
                    self.audio.set_window_open(true);
                    return open_task.discard().chain(window::gain_focus(id));
                }
                DaemonEvent::TrayToggleWindow => {
                    if let Some(id) = self.main_window {
                        if self
                            .last_window_opened_at
                            .is_some_and(|opened| opened.elapsed() < Duration::from_millis(350))
                        {
                            return window::gain_focus(id);
                        }
                        return self.hide_window_to_tray(id);
                    }
                    let (id, open_task) = window::open(crate::main_window_settings());
                    self.main_window = Some(id);
                    self.last_window_opened_at = Some(Instant::now());
                    self.audio.set_window_open(true);
                    return open_task.discard().chain(window::gain_focus(id));
                }
                DaemonEvent::TrayQuit => {
                    return self.shutdown_and_exit();
                }
                DaemonEvent::AudioPlaybackChanged => {
                    let previewing = self.audio.is_previewing();
                    if let Some(modal) = &mut self.modal {
                        modal.refresh_labels(self.config.global_volume, previewing);
                    }
                }
                DaemonEvent::AudioError(err) => {
                    if let Some(modal) = &mut self.modal {
                        modal.error_hint = Some(err);
                        modal.refresh_labels(self.config.global_volume, self.audio.is_previewing());
                    } else if let Some(settings) = &mut self.settings_modal {
                        settings.status_message = Some(err);
                        settings.status_is_error = true;
                        settings
                            .refresh_labels(self.config.monitor_volume, self.config.cable_volume);
                    }
                }
                DaemonEvent::HotkeyError(err) => {
                    self.last_hotkey_error = Some(err.clone());
                    if let Some(modal) = &mut self.modal
                        && modal.error_hint.is_none()
                    {
                        modal.error_hint = Some(err);
                        modal.refresh_labels(self.config.global_volume, self.audio.is_previewing());
                    }
                }
                DaemonEvent::DevicesRefreshed {
                    microphones,
                    virtual_outputs,
                    monitor_outputs,
                } => {
                    self.microphones = microphones;
                    self.outputs = virtual_outputs;
                    self.monitor_outputs = monitor_outputs;
                    if let Some(settings) = &mut self.settings_modal {
                        settings.status_message = Some("Audio device list refreshed".to_string());
                        settings.status_is_error = false;
                        settings
                            .refresh_labels(self.config.monitor_volume, self.config.cable_volume);
                    }
                }
            },
            Message::PadClicked(idx) => {
                let has_sound = self
                    .config
                    .slots
                    .get(idx)
                    .and_then(|s| s.stored_file.as_deref())
                    .is_some();
                if has_sound {
                    self.audio.toggle_slot(idx);
                } else {
                    self.open_modal(idx);
                }
            }
            Message::OpenSlotModal(idx) => {
                self.open_modal(idx);
            }
            Message::ModalBackdropClicked => {
                let Some(modal) = &mut self.modal else {
                    return Task::none();
                };
                if modal.show_unsaved_prompt {
                    return Task::none();
                }
                if modal.is_dirty() {
                    self.audio.stop_preview();
                    modal.is_recording_keybind = false;
                    self.hotkeys.set_suspended(false);
                    modal.show_unsaved_prompt = true;
                } else {
                    self.audio.stop_preview();
                    self.hotkeys.set_suspended(false);
                    self.modal = None;
                }
            }
            Message::ModalKeepEditing => {
                if let Some(modal) = &mut self.modal {
                    modal.show_unsaved_prompt = false;
                    modal.refresh_labels(self.config.global_volume, self.audio.is_previewing());
                }
            }
            Message::ModalDiscardChanges | Message::ModalCancel => {
                self.audio.stop_preview();
                self.hotkeys.set_suspended(false);
                self.modal = None;
            }
            Message::ModalNameChanged(name) => {
                if let Some(modal) = &mut self.modal {
                    modal.draft.name = name;
                    modal.error_hint = None;
                }
            }
            Message::ModalPickAudioFile => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Select Audio File")
                            .add_filter(
                                "Audio Files",
                                &[
                                    "mp3", "wav", "ogg", "flac", "m4a", "aac", "aiff", "caf",
                                    "mkv", "webm",
                                ],
                            )
                            .pick_file()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    },
                    Message::ModalAudioFilePicked,
                );
            }
            Message::ModalAudioFilePicked(picked) => {
                if let Some(path) = picked
                    && let Some(modal) = &mut self.modal
                {
                    self.audio.stop_preview();
                    match storage::validate_audio_file(&path) {
                        Ok(probe) => {
                            if modal.draft.name.trim().is_empty()
                                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                            {
                                modal.draft.name = stem.to_string();
                            }
                            let file_name = path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("audio")
                                .to_string();
                            modal.draft.display_file = Some(file_name);
                            modal.draft.duration_ms = probe.duration_ms;
                            modal.draft.audio_format = Some(probe.format_name);
                            modal.pending_source_path = Some(path);
                            modal.error_hint = None;
                        }
                        Err(err) => {
                            modal.error_hint = Some(err);
                        }
                    }
                    modal.refresh_labels(self.config.global_volume, false);
                }
            }
            Message::ModalVolumeChanged(vol) => {
                if let Some(modal) = &mut self.modal {
                    modal.draft.custom_volume = Some(vol);
                    let previewing = self.audio.is_previewing();
                    if previewing {
                        self.audio
                            .update_preview(vol, modal.draft.pitch_multiplier());
                    }
                    modal.refresh_labels(self.config.global_volume, previewing);
                }
            }
            Message::ModalUseGlobalVolume => {
                if let Some(modal) = &mut self.modal {
                    modal.draft.custom_volume = None;
                    let previewing = self.audio.is_previewing();
                    if previewing {
                        self.audio.update_preview(
                            self.config.global_volume,
                            modal.draft.pitch_multiplier(),
                        );
                    }
                    modal.refresh_labels(self.config.global_volume, previewing);
                }
            }
            Message::ModalPitchChanged(pitch) => {
                if let Some(modal) = &mut self.modal {
                    modal.draft.pitch_percent = pitch;
                    let previewing = self.audio.is_previewing();
                    if previewing {
                        let eff_vol = modal.draft.effective_volume(self.config.global_volume);
                        self.audio
                            .update_preview(eff_vol, modal.draft.pitch_multiplier());
                    }
                    modal.refresh_labels(self.config.global_volume, previewing);
                }
            }
            Message::ModalResetPitch => {
                if let Some(modal) = &mut self.modal {
                    modal.draft.pitch_percent = DEFAULT_PITCH_PERCENT;
                    let previewing = self.audio.is_previewing();
                    if previewing {
                        let eff_vol = modal.draft.effective_volume(self.config.global_volume);
                        self.audio
                            .update_preview(eff_vol, modal.draft.pitch_multiplier());
                    }
                    modal.refresh_labels(self.config.global_volume, previewing);
                }
            }
            Message::ModalTogglePreview => {
                if let Some(modal) = &mut self.modal
                    && let Some(path) = modal.resolve_audio_path()
                {
                    let vol = modal.draft.effective_volume(self.config.global_volume);
                    let pitch = modal.draft.pitch_multiplier();
                    modal.error_hint = None;
                    self.audio.toggle_preview(path, vol, pitch);
                }
            }
            Message::ModalToggleRecordKeybind => {
                if let Some(modal) = &mut self.modal {
                    modal.is_recording_keybind = !modal.is_recording_keybind;
                    self.hotkeys.set_suspended(modal.is_recording_keybind);
                    modal.refresh_labels(self.config.global_volume, self.audio.is_previewing());
                }
            }
            Message::ModalClearKeybind => {
                if let Some(modal) = &mut self.modal {
                    modal.draft.keybind = None;
                    modal.conflicting_slot = None;
                    modal.is_recording_keybind = false;
                    self.hotkeys.set_suspended(false);
                    modal.refresh_labels(self.config.global_volume, self.audio.is_previewing());
                }
            }
            Message::ModalClearSlot => {
                if let Some(modal) = self.modal.take() {
                    self.audio.stop_preview();
                    self.audio.stop_slot(modal.slot_index);
                    self.hotkeys.set_suspended(false);
                    let previous_file = self
                        .config
                        .slots
                        .get(modal.slot_index)
                        .and_then(|s| s.stored_file.clone());
                    if let Some(slot) = self.config.slots.get_mut(modal.slot_index) {
                        *slot = SlotConfig::default();
                    }
                    if let Some(old_file) = previous_file {
                        self.cleanup_replaced_sound(&old_file);
                    }
                    self.rebuild_slot_label(modal.slot_index);
                    self.sync_runtime_slots();
                    let _ = storage::save_config(&self.config);
                }
            }
            Message::ModalSave => {
                self.commit_modal_save();
            }
            Message::KeyCaptured(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                repeat: false,
                ..
            }) => {
                if let Some(modal) = &mut self.modal
                    && modal.is_recording_keybind
                {
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) && modifiers.is_empty() {
                        modal.is_recording_keybind = false;
                        self.hotkeys.set_suspended(false);
                        modal.refresh_labels(self.config.global_volume, self.audio.is_previewing());
                    } else if let Some(combo) =
                        hotkeys::capture_iced_keybind(&physical_key, &key, modifiers)
                    {
                        modal.conflicting_slot = hotkeys::find_conflicting_slot(
                            &self.config.slots,
                            modal.slot_index,
                            &combo,
                        );
                        modal.draft.keybind = Some(combo);
                        modal.is_recording_keybind = false;
                        self.hotkeys.set_suspended(false);
                        modal.refresh_labels(self.config.global_volume, self.audio.is_previewing());
                    }
                }
            }
            Message::KeyCaptured(_) => {}
            Message::SelectMicrophone(mic) => {
                self.audio.set_microphone(&mic);
                self.config.microphone = Some(mic);
                let _ = storage::save_config(&self.config);
            }
            Message::SelectOutput(out) => {
                self.audio.set_virtual_output(&out);
                self.config.output_device = Some(out);
                let _ = storage::save_config(&self.config);
            }
            Message::ShrinkGrid => {
                if self.config.grid_rows > MIN_GRID_ROWS {
                    self.config.grid_rows -= 1;
                    let visible = self.visible_slot_count();
                    for idx in visible..self.config.slots.len() {
                        self.audio.stop_slot(idx);
                    }
                    self.update_grid_label();
                    self.sync_runtime_slots();
                    let _ = storage::save_config(&self.config);
                }
            }
            Message::GrowGrid => {
                if self.config.grid_rows < MAX_GRID_ROWS {
                    self.config.grid_rows += 1;
                    let needed = self.visible_slot_count();
                    if self.config.slots.len() < needed {
                        self.config.slots.resize_with(needed, SlotConfig::default);
                    }
                    self.rebuild_all_labels();
                    self.sync_runtime_slots();
                    let _ = storage::save_config(&self.config);
                }
            }
            Message::ToggleCloseToTray(enabled) => {
                self.config.close_to_tray = enabled;
                self.tray.set_enabled(enabled);
                let _ = storage::save_config(&self.config);
            }
            Message::GlobalVolumeChanged(vol) => {
                self.config.global_volume = vol;
                self.audio.set_global_volume(vol);
                self.global_volume_label = format!("{vol}%");
                for idx in 0..self.config.slots.len() {
                    if self.config.slots[idx].custom_volume.is_none() {
                        self.rebuild_slot_label(idx);
                    }
                }
            }
            Message::CommitGlobalVolume => {
                let _ = storage::save_config(&self.config);
            }
            Message::StopAll => {
                self.audio.stop_all();
            }
            Message::OpenSettings => {
                self.audio.stop_preview();
                self.hotkeys.set_suspended(false);
                self.modal = None;
                self.settings_modal = Some(SettingsModalState::new(
                    self.config.monitor_volume,
                    self.config.cable_volume,
                ));
            }
            Message::SettingsBackdropClicked => {
                if let Some(settings) = &self.settings_modal
                    && settings.show_delete_confirm
                {
                    return Task::none();
                }
                self.settings_modal = None;
            }
            Message::SettingsClose => {
                self.settings_modal = None;
            }
            Message::SettingsRefreshDevices => {
                self.audio.refresh_devices();
            }
            Message::SettingsSelectMonitor(monitor) => {
                self.audio.set_monitor_output(&monitor);
                self.config.monitor_device = Some(monitor);
                let _ = storage::save_config(&self.config);
            }
            Message::SettingsMonitorVolumeChanged(vol) => {
                self.config.monitor_volume = vol;
                self.audio.set_monitor_volume(vol);
                if let Some(settings) = &mut self.settings_modal {
                    settings.refresh_labels(self.config.monitor_volume, self.config.cable_volume);
                }
            }
            Message::SettingsCableVolumeChanged(vol) => {
                self.config.cable_volume = vol;
                self.audio.set_cable_volume(vol);
                if let Some(settings) = &mut self.settings_modal {
                    settings.refresh_labels(self.config.monitor_volume, self.config.cable_volume);
                }
            }
            Message::SettingsCommitStreamVolumes => {
                let _ = storage::save_config(&self.config);
            }
            Message::SettingsExportBackup => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Export Broma Library Backup")
                            .set_file_name("broma-backup.bromapack")
                            .add_filter("Broma Backup", &["bromapack"])
                            .save_file()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    },
                    Message::SettingsExportBackupPicked,
                );
            }
            Message::SettingsExportBackupPicked(picked) => {
                if let Some(path) = picked {
                    let res = storage::export_backup(&path, &self.config);
                    if let Some(settings) = &mut self.settings_modal {
                        match res {
                            Ok(count) => {
                                let noun = if count == 1 { "file" } else { "files" };
                                settings.status_message =
                                    Some(format!("Exported backup ({count} sound {noun})"));
                                settings.status_is_error = false;
                            }
                            Err(err) => {
                                settings.status_message = Some(err);
                                settings.status_is_error = true;
                            }
                        }
                        settings
                            .refresh_labels(self.config.monitor_volume, self.config.cable_volume);
                    }
                }
            }
            Message::SettingsImportBackup => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Import Broma Library Backup")
                            .add_filter("Broma Backup", &["bromapack"])
                            .pick_file()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    },
                    Message::SettingsImportBackupPicked,
                );
            }
            Message::SettingsImportBackupPicked(picked) => {
                if let Some(path) = picked {
                    self.audio.stop_all();
                    self.audio.clear_cache();
                    match storage::import_backup(&path) {
                        Ok(mut imported) => {
                            let mut adjusted_devices = false;
                            if !imported
                                .microphone
                                .as_ref()
                                .is_some_and(|m| self.microphones.contains(m))
                            {
                                imported.microphone = self.config.microphone.clone();
                                adjusted_devices = true;
                            }
                            if !imported
                                .output_device
                                .as_ref()
                                .is_some_and(|o| self.outputs.contains(o))
                            {
                                imported.output_device = self.config.output_device.clone();
                                adjusted_devices = true;
                            }
                            if !imported
                                .monitor_device
                                .as_ref()
                                .is_some_and(|m| self.monitor_outputs.contains(m))
                            {
                                imported.monitor_device = self.config.monitor_device.clone();
                                adjusted_devices = true;
                            }

                            if imported.microphone != self.config.microphone
                                && let Some(mic) = imported.microphone.as_deref()
                            {
                                self.audio.set_microphone(mic);
                            }
                            if imported.output_device != self.config.output_device
                                && let Some(out) = imported.output_device.as_deref()
                            {
                                self.audio.set_virtual_output(out);
                            }
                            if imported.monitor_device != self.config.monitor_device
                                && let Some(mon) = imported.monitor_device.as_deref()
                            {
                                self.audio.set_monitor_output(mon);
                            }

                            self.audio.set_global_volume(imported.global_volume);
                            self.audio.set_monitor_volume(imported.monitor_volume);
                            self.audio.set_cable_volume(imported.cable_volume);
                            self.tray.set_enabled(imported.close_to_tray);
                            self.config = imported;
                            if adjusted_devices {
                                let _ = storage::save_config(&self.config);
                            }
                            self.last_hotkey_error = None;
                            self.rebuild_all_labels();
                            self.sync_runtime_slots();

                            if let Some(settings) = &mut self.settings_modal {
                                settings.refresh_stats();
                                settings.status_message =
                                    Some("Library backup restored".to_string());
                                settings.status_is_error = false;
                                settings.refresh_labels(
                                    self.config.monitor_volume,
                                    self.config.cable_volume,
                                );
                            }
                        }
                        Err(err) => {
                            if let Some(settings) = &mut self.settings_modal {
                                settings.status_message = Some(err);
                                settings.status_is_error = true;
                                settings.refresh_labels(
                                    self.config.monitor_volume,
                                    self.config.cable_volume,
                                );
                            }
                        }
                    }
                }
            }
            Message::SettingsRequestDeleteAll => {
                if let Some(settings) = &mut self.settings_modal {
                    settings.show_delete_confirm = true;
                }
            }
            Message::SettingsCancelDeleteAll => {
                if let Some(settings) = &mut self.settings_modal {
                    settings.show_delete_confirm = false;
                }
            }
            Message::SettingsConfirmDeleteAll => {
                self.audio.stop_all();
                self.audio.clear_cache();
                match storage::delete_all_data() {
                    Ok(()) => {
                        let prev_mic = self
                            .config
                            .microphone
                            .take()
                            .filter(|m| self.microphones.contains(m));
                        let prev_out = self
                            .config
                            .output_device
                            .take()
                            .filter(|o| self.outputs.contains(o));
                        let prev_mon = self
                            .config
                            .monitor_device
                            .take()
                            .filter(|m| self.monitor_outputs.contains(m));

                        self.config = AppConfig {
                            microphone: prev_mic,
                            output_device: prev_out,
                            monitor_device: prev_mon,
                            ..AppConfig::default()
                        };

                        self.audio.set_global_volume(self.config.global_volume);
                        self.audio.set_monitor_volume(self.config.monitor_volume);
                        self.audio.set_cable_volume(self.config.cable_volume);
                        self.tray.set_enabled(self.config.close_to_tray);
                        self.last_hotkey_error = None;
                        self.rebuild_all_labels();
                        self.sync_runtime_slots();

                        if let Some(settings) = &mut self.settings_modal {
                            settings.show_delete_confirm = false;
                            settings.refresh_stats();
                            settings.status_message =
                                Some("All saved library data deleted".to_string());
                            settings.status_is_error = false;
                            settings.refresh_labels(
                                self.config.monitor_volume,
                                self.config.cable_volume,
                            );
                        }
                    }
                    Err(err) => {
                        if let Some(settings) = &mut self.settings_modal {
                            settings.show_delete_confirm = false;
                            settings.status_message = Some(err);
                            settings.status_is_error = true;
                            settings.refresh_labels(
                                self.config.monitor_volume,
                                self.config.cable_volume,
                            );
                        }
                    }
                }
            }
        }
        Task::none()
    }

    pub fn view(&self, _window: window::Id) -> Element<'_, Message> {
        ui::view_app(self)
    }

    pub fn visible_slot_count(&self) -> usize {
        self.config.grid_rows * GRID_COLS
    }

    fn hide_window_to_tray(&mut self, id: window::Id) -> Task<Message> {
        self.audio.stop_preview();
        self.hotkeys.set_suspended(false);
        self.modal = None;
        self.settings_modal = None;
        if self.main_window == Some(id) {
            self.main_window = None;
        }
        self.last_window_opened_at = None;
        self.audio.set_window_open(false);
        window::close(id)
    }

    /// Deterministically shuts down hotkeys, tray, and the background audio
    /// service (unloading any virtual microphone) before terminating `iced`.
    fn shutdown_and_exit(&mut self) -> Task<Message> {
        self.audio.set_window_open(false);
        self.hotkeys.shutdown();
        self.tray.shutdown();
        self.audio_guard.shutdown();
        iced::exit()
    }

    fn sync_runtime_slots(&mut self) {
        let visible = self.visible_slot_count();
        self.audio
            .sync_slots(&self.config.slots, visible, self.config.global_volume);
        self.hotkeys.sync_slots(&self.config.slots, visible);
    }

    fn open_modal(&mut self, idx: usize) {
        let Some(current) = self.config.slots.get(idx).cloned() else {
            return;
        };
        self.audio.stop_preview();
        self.hotkeys.set_suspended(false);
        self.settings_modal = None;
        let mut modal = ModalState::new(idx, current, self.config.global_volume);
        if modal.draft.keybind.is_some()
            && let Some(err) = &self.last_hotkey_error
        {
            modal.error_hint = Some(err.clone());
            modal.refresh_labels(self.config.global_volume, false);
        }
        self.modal = Some(modal);
    }

    fn commit_modal_save(&mut self) {
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        self.audio.stop_preview();
        self.audio.stop_slot(modal.slot_index);
        self.hotkeys.set_suspended(false);

        if let Some(src_path) = &modal.pending_source_path {
            match storage::import_audio_copy(src_path) {
                Ok(imported) => {
                    modal.draft.stored_file = Some(imported.stored_file);
                    modal.draft.display_file = Some(imported.display_file);
                    modal.draft.duration_ms = imported.duration_ms;
                    modal.draft.audio_format = Some(imported.audio_format);
                }
                Err(err) => {
                    modal.error_hint = Some(err);
                    modal.show_unsaved_prompt = false;
                    modal.refresh_labels(self.config.global_volume, false);
                    self.modal = Some(modal);
                    return;
                }
            }
        }

        if modal.draft.name.trim().is_empty()
            && let Some(display) = &modal.draft.display_file
        {
            modal.draft.name = display.clone();
        }

        let previous_file = self
            .config
            .slots
            .get(modal.slot_index)
            .and_then(|s| s.stored_file.clone());

        if let Some(kb) = modal.draft.keybind.as_deref() {
            while let Some(dup_idx) =
                hotkeys::find_conflicting_slot(&self.config.slots, modal.slot_index, kb)
            {
                self.config.slots[dup_idx].keybind = None;
                self.rebuild_slot_label(dup_idx);
            }
        }

        if let Some(slot) = self.config.slots.get_mut(modal.slot_index) {
            *slot = modal.draft;
        }
        if let Some(old_file) = previous_file {
            self.cleanup_replaced_sound(&old_file);
        }
        self.rebuild_slot_label(modal.slot_index);
        self.last_hotkey_error = None;
        self.sync_runtime_slots();
        let _ = storage::save_config(&self.config);
    }

    fn cleanup_replaced_sound(&mut self, old_stored_file: &str) {
        let still_used = self
            .config
            .slots
            .iter()
            .any(|s| s.stored_file.as_deref() == Some(old_stored_file));
        if !still_used {
            if let Some(path) = storage::resolve_stored_sound(old_stored_file) {
                self.audio.invalidate_cached_file(path);
            }
            storage::remove_sound_if_unreferenced(&self.config.slots, old_stored_file);
        }
    }

    fn rebuild_all_labels(&mut self) {
        self.update_grid_label();
        self.global_volume_label = format!("{}%", self.config.global_volume);

        self.pad_views.clear();
        self.pad_views.reserve(self.config.slots.len());
        for slot in &self.config.slots {
            self.pad_views
                .push(build_pad_view(slot, self.config.global_volume));
        }
    }

    fn rebuild_slot_label(&mut self, idx: usize) {
        if let Some(slot) = self.config.slots.get(idx) {
            let view_item = build_pad_view(slot, self.config.global_volume);
            if let Some(existing) = self.pad_views.get_mut(idx) {
                *existing = view_item;
            }
        }
    }

    fn update_grid_label(&mut self) {
        let total = self.visible_slot_count();
        self.grid_label = format!("{GRID_COLS}x{} ({total})", self.config.grid_rows);
    }
}

fn build_pad_view(slot: &SlotConfig, global_volume: u8) -> PadViewItem {
    if slot.is_empty() {
        return PadViewItem {
            is_empty: true,
            has_file: false,
            title: String::new(),
            meta_label: String::new(),
            keybind_label: String::new(),
        };
    }

    let has_file = slot.stored_file.is_some();
    let title = if slot.name.trim().is_empty() {
        "Untitled".to_string()
    } else {
        slot.name.clone()
    };

    let meta_label = if !has_file {
        "No audio file — click to configure".to_string()
    } else {
        let vol_str = match slot.custom_volume {
            Some(v) => format!("Vol {v}%"),
            None => format!("Global ({global_volume}%)"),
        };
        if let Some(ms) = slot.duration_ms {
            let secs = (ms as f64) / 1000.0;
            format!("{secs:.1}s · {vol_str} · {:.2}x", slot.pitch_multiplier())
        } else {
            format!("{vol_str} · {:.2}x", slot.pitch_multiplier())
        }
    };

    let keybind_label = match &slot.keybind {
        Some(kb) => format!("[{kb}]"),
        None => "No keybind".to_string(),
    };

    PadViewItem {
        is_empty: false,
        has_file,
        title,
        meta_label,
        keybind_label,
    }
}
