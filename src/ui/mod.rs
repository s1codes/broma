// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Root UI composition for Broma.

pub mod components;
pub mod theme;

use iced::widget::{column, container, stack};
use iced::{Element, Length};

use crate::state::{App, Message};
use crate::storage::{MAX_GRID_ROWS, MIN_GRID_ROWS};
use crate::ui::components::config_modal::{ConfigModalProps, config_modal};
use crate::ui::components::footer_bar::{FooterBarProps, footer_bar};
use crate::ui::components::pad_grid::pad_grid;
use crate::ui::components::settings_modal::{SettingsModalProps, settings_modal};

pub fn view_app(app: &App) -> Element<'_, Message> {
    let visible_count = app.visible_slot_count().min(app.pad_views.len());
    let visible_pads = &app.pad_views[..visible_count];

    let grid_section = pad_grid(
        visible_pads,
        app.audio.active_slots_mask(),
        Message::PadClicked,
        Message::OpenSlotModal,
    );

    let footer_section = footer_bar(FooterBarProps {
        microphones: &app.microphones,
        selected_microphone: app.config.microphone.as_ref(),
        outputs: &app.outputs,
        selected_output: app.config.output_device.as_ref(),
        grid_label: &app.grid_label,
        can_shrink_grid: app.config.grid_rows > MIN_GRID_ROWS,
        can_grow_grid: app.config.grid_rows < MAX_GRID_ROWS,
        close_to_tray: app.config.close_to_tray,
        global_volume: app.config.global_volume,
        global_volume_label: &app.global_volume_label,
        on_select_microphone: Message::SelectMicrophone,
        on_select_output: Message::SelectOutput,
        on_shrink_grid: Message::ShrinkGrid,
        on_grow_grid: Message::GrowGrid,
        on_toggle_close_to_tray: Message::ToggleCloseToTray,
        on_global_volume_change: Message::GlobalVolumeChanged,
        on_global_volume_commit: Message::CommitGlobalVolume,
        on_stop_all: Message::StopAll,
        on_open_settings: Message::OpenSettings,
    });

    let base_screen = container(
        column![grid_section, footer_section]
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::main_container);

    if let Some(modal) = &app.modal {
        let slider_volume = modal.draft.effective_volume(app.config.global_volume);
        let can_clear_slot = !modal.original.is_empty() || modal.is_dirty();

        let overlay = config_modal(ConfigModalProps {
            modal_title: &modal.title_label,
            name: &modal.draft.name,
            file_label: &modal.file_label,
            has_file: modal.has_file(),
            slider_volume,
            has_custom_volume: modal.draft.custom_volume.is_some(),
            volume_label: &modal.volume_label,
            pitch_percent: modal.draft.pitch_percent,
            pitch_label: &modal.pitch_label,
            is_previewing: app.audio.is_previewing(),
            has_error: modal.error_hint.is_some(),
            preview_status_label: &modal.preview_status_label,
            keybind_label: &modal.keybind_label,
            has_keybind: modal.draft.keybind.is_some(),
            is_recording_keybind: modal.is_recording_keybind,
            can_clear_slot,
            show_unsaved_prompt: modal.show_unsaved_prompt,
            on_backdrop_click: Message::ModalBackdropClicked,
            on_name_change: Message::ModalNameChanged,
            on_pick_file: Message::ModalPickAudioFile,
            on_volume_change: Message::ModalVolumeChanged,
            on_use_global_volume: Message::ModalUseGlobalVolume,
            on_pitch_change: Message::ModalPitchChanged,
            on_reset_pitch: Message::ModalResetPitch,
            on_toggle_preview: Message::ModalTogglePreview,
            on_toggle_record_keybind: Message::ModalToggleRecordKeybind,
            on_clear_keybind: Message::ModalClearKeybind,
            on_clear_slot: Message::ModalClearSlot,
            on_cancel: Message::ModalCancel,
            on_save: Message::ModalSave,
            on_keep_editing: Message::ModalKeepEditing,
            on_discard_changes: Message::ModalDiscardChanges,
        });

        return stack![base_screen, overlay]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    }

    if let Some(settings) = &app.settings_modal {
        let overlay = settings_modal(SettingsModalProps {
            monitor_outputs: &app.monitor_outputs,
            selected_monitor: app.config.monitor_device.as_ref(),
            monitor_volume: app.config.monitor_volume,
            monitor_volume_label: &settings.monitor_volume_label,
            cable_volume: app.config.cable_volume,
            cable_volume_label: &settings.cable_volume_label,
            library_stats_label: &settings.library_stats_label,
            status_label: &settings.status_label,
            has_error: settings.status_is_error,
            show_delete_confirm: settings.show_delete_confirm,
            on_backdrop_click: Message::SettingsBackdropClicked,
            on_refresh_devices: Message::SettingsRefreshDevices,
            on_select_monitor: Message::SettingsSelectMonitor,
            on_monitor_volume_change: Message::SettingsMonitorVolumeChanged,
            on_cable_volume_change: Message::SettingsCableVolumeChanged,
            on_commit_stream_volumes: Message::SettingsCommitStreamVolumes,
            on_export_backup: Message::SettingsExportBackup,
            on_import_backup: Message::SettingsImportBackup,
            on_request_delete_all: Message::SettingsRequestDeleteAll,
            on_cancel_delete_all: Message::SettingsCancelDeleteAll,
            on_confirm_delete_all: Message::SettingsConfirmDeleteAll,
            on_close: Message::SettingsClose,
        });

        return stack![base_screen, overlay]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    }

    base_screen.into()
}
