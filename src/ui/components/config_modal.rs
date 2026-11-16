// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Slot configuration modal (Name, Audio File, Volume, Pitch, Preview, Keybind)
//! plus the "Save changes?" confirmation reminder when clicking the backdrop
//! with unsaved edits.

use iced::widget::{
    button, column, container, mouse_area, opaque, row, rule, slider, space, text, text_input,
};
use iced::{Alignment, Element, Length, Padding};

use crate::storage::{DEFAULT_PITCH_PERCENT, MAX_PITCH_PERCENT, MIN_PITCH_PERCENT};
use crate::ui::theme;

pub struct ConfigModalProps<'a, Message> {
    pub modal_title: &'a str,
    pub name: &'a str,
    pub file_label: &'a str,
    pub has_file: bool,

    pub slider_volume: u8,
    pub has_custom_volume: bool,
    pub volume_label: &'a str,

    pub pitch_percent: u16,
    pub pitch_label: &'a str,

    pub is_previewing: bool,
    pub has_error: bool,
    pub preview_status_label: &'a str,

    pub keybind_label: &'a str,
    pub has_keybind: bool,
    pub is_recording_keybind: bool,

    pub can_clear_slot: bool,
    pub show_unsaved_prompt: bool,

    pub on_backdrop_click: Message,
    pub on_name_change: fn(String) -> Message,
    pub on_pick_file: Message,
    pub on_volume_change: fn(u8) -> Message,
    pub on_use_global_volume: Message,
    pub on_pitch_change: fn(u16) -> Message,
    pub on_reset_pitch: Message,
    pub on_toggle_preview: Message,
    pub on_toggle_record_keybind: Message,
    pub on_clear_keybind: Message,
    pub on_clear_slot: Message,
    pub on_cancel: Message,
    pub on_save: Message,

    pub on_keep_editing: Message,
    pub on_discard_changes: Message,
}

pub fn config_modal<'a, Message: Clone + 'a>(
    props: ConfigModalProps<'a, Message>,
) -> Element<'a, Message> {
    let backdrop_msg = props.on_backdrop_click;

    let dialog_content: Element<'a, Message> = if props.show_unsaved_prompt {
        unsaved_changes_card(
            props.on_keep_editing,
            props.on_discard_changes,
            props.on_save,
        )
    } else {
        editor_card(
            props.modal_title,
            props.name,
            props.file_label,
            props.has_file,
            props.slider_volume,
            props.has_custom_volume,
            props.volume_label,
            props.pitch_percent,
            props.pitch_label,
            props.is_previewing,
            props.has_error,
            props.preview_status_label,
            props.keybind_label,
            props.has_keybind,
            props.is_recording_keybind,
            props.can_clear_slot,
            props.on_name_change,
            props.on_pick_file,
            props.on_volume_change,
            props.on_use_global_volume,
            props.on_pitch_change,
            props.on_reset_pitch,
            props.on_toggle_preview,
            props.on_toggle_record_keybind,
            props.on_clear_keybind,
            props.on_clear_slot,
            props.on_cancel,
            props.on_save,
        )
    };

    let backdrop = mouse_area(
        container(opaque(dialog_content))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(theme::scrim_container),
    )
    .on_press(backdrop_msg);

    opaque(backdrop)
}

#[allow(clippy::too_many_arguments)]
fn editor_card<'a, Message: Clone + 'a>(
    modal_title: &'a str,
    name: &'a str,
    file_label: &'a str,
    has_file: bool,
    slider_volume: u8,
    has_custom_volume: bool,
    volume_label: &'a str,
    pitch_percent: u16,
    pitch_label: &'a str,
    is_previewing: bool,
    has_error: bool,
    preview_status_label: &'a str,
    keybind_label: &'a str,
    has_keybind: bool,
    is_recording_keybind: bool,
    can_clear_slot: bool,
    on_name_change: fn(String) -> Message,
    on_pick_file: Message,
    on_volume_change: fn(u8) -> Message,
    on_use_global_volume: Message,
    on_pitch_change: fn(u16) -> Message,
    on_reset_pitch: Message,
    on_toggle_preview: Message,
    on_toggle_record_keybind: Message,
    on_clear_keybind: Message,
    on_clear_slot: Message,
    on_cancel: Message,
    on_save: Message,
) -> Element<'a, Message> {
    // A. Set Name
    let name_section = column![
        text("Name").size(12.5).color(theme::TEXT_DIM),
        text_input("Button label...", name)
            .on_input(on_name_change)
            .size(13.5)
            .padding([8.0, 10.0])
            .style(theme::text_input_style),
    ]
    .spacing(5.0);

    // B. Native Audio File Chooser
    let file_section = column![
        text("Audio File").size(12.5).color(theme::TEXT_DIM),
        row![
            container(text(file_label).size(12.5).color(if has_file {
                theme::TEXT_MAIN
            } else {
                theme::TEXT_MUTED
            }),)
            .width(Length::Fill)
            .padding([8.0, 10.0])
            .style(theme::subtle_well_container),
            button(text("Choose File...").size(13.0))
                .padding([8.0, 12.0])
                .on_press(on_pick_file)
                .style(theme::btn_secondary),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center),
    ]
    .spacing(5.0);

    // C. Volume (per-button or follows Global Volume)
    let mut vol_header = row![
        text("Volume").size(12.5).color(theme::TEXT_DIM),
        space::horizontal(),
        text(volume_label).size(12.0).color(theme::TEXT_MAIN),
    ]
    .spacing(8.0)
    .align_y(Alignment::Center);

    if has_custom_volume {
        vol_header = vol_header.push(
            button(text("Use Global").size(11.5))
                .padding([3.0, 8.0])
                .on_press(on_use_global_volume)
                .style(theme::btn_small_chip),
        );
    }

    let volume_section = column![
        vol_header,
        slider(0..=100, slider_volume, on_volume_change).style(theme::slider_style),
    ]
    .spacing(5.0);

    // D. Pitch
    let mut pitch_header = row![
        text("Pitch").size(12.5).color(theme::TEXT_DIM),
        space::horizontal(),
        text(pitch_label).size(12.0).color(theme::TEXT_MAIN),
    ]
    .spacing(8.0)
    .align_y(Alignment::Center);

    if pitch_percent != DEFAULT_PITCH_PERCENT {
        pitch_header = pitch_header.push(
            button(text("Reset").size(11.5))
                .padding([3.0, 8.0])
                .on_press(on_reset_pitch)
                .style(theme::btn_small_chip),
        );
    }

    let pitch_section = column![
        pitch_header,
        slider(
            MIN_PITCH_PERCENT..=MAX_PITCH_PERCENT,
            pitch_percent,
            on_pitch_change,
        )
        .step(5u16)
        .default(DEFAULT_PITCH_PERCENT)
        .style(theme::slider_style),
    ]
    .spacing(5.0);

    // E. Preview
    let mut preview_btn = button(
        text(if is_previewing {
            "Stop Preview"
        } else {
            "Preview"
        })
        .size(13.0),
    )
    .padding([7.0, 14.0])
    .style(if is_previewing {
        theme::btn_danger
    } else {
        theme::btn_secondary
    });

    if has_file {
        preview_btn = preview_btn.on_press(on_toggle_preview);
    }

    let preview_section = row![
        preview_btn,
        text(preview_status_label).size(12.0).color(if has_error {
            theme::DANGER_HOVER
        } else if is_previewing {
            theme::LIVE
        } else {
            theme::TEXT_MUTED
        }),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);

    // F. Optional Keybind
    let mut keybind_row = row![
        container(
            text(keybind_label)
                .size(12.5)
                .color(if is_recording_keybind {
                    theme::ACCENT
                } else if has_keybind {
                    theme::TEXT_MAIN
                } else {
                    theme::TEXT_MUTED
                })
        )
        .width(Length::Fill)
        .padding([7.0, 10.0])
        .style(theme::subtle_well_container),
        button(
            text(if is_recording_keybind {
                "Cancel Key"
            } else {
                "Set Keybind"
            })
            .size(12.5),
        )
        .padding([7.0, 12.0])
        .on_press(on_toggle_record_keybind)
        .style(theme::btn_secondary),
    ]
    .spacing(8.0)
    .align_y(Alignment::Center);

    if has_keybind && !is_recording_keybind {
        keybind_row = keybind_row.push(
            button(text("Clear").size(12.5))
                .padding([7.0, 10.0])
                .on_press(on_clear_keybind)
                .style(theme::btn_small_chip),
        );
    }

    let keybind_section = column![
        text("Keybind (Optional)").size(12.5).color(theme::TEXT_DIM),
        keybind_row,
    ]
    .spacing(5.0);

    // Bottom Actions (Cancel & Save on bottom-right)
    let mut bottom_actions = row![].spacing(8.0).align_y(Alignment::Center);
    if can_clear_slot {
        bottom_actions = bottom_actions.push(
            button(text("Clear Slot").size(12.5))
                .padding([7.0, 12.0])
                .on_press(on_clear_slot)
                .style(theme::btn_small_chip),
        );
    }
    bottom_actions = bottom_actions
        .push(space::horizontal())
        .push(
            button(text("Cancel").size(13.0))
                .padding([8.0, 16.0])
                .on_press(on_cancel)
                .style(theme::btn_secondary),
        )
        .push(
            button(text("Save").size(13.0))
                .padding([8.0, 18.0])
                .on_press(on_save)
                .style(theme::btn_primary),
        );

    container(
        column![
            text(modal_title).size(16.0).color(theme::TEXT_MAIN),
            rule::horizontal(1).style(theme::divider_rule),
            name_section,
            file_section,
            volume_section,
            pitch_section,
            preview_section,
            keybind_section,
            rule::horizontal(1).style(theme::divider_rule),
            bottom_actions,
        ]
        .spacing(12.0),
    )
    .width(Length::Fixed(440.0))
    .padding(Padding::new(18.0))
    .style(theme::modal_container)
    .into()
}

fn unsaved_changes_card<'a, Message: Clone + 'a>(
    on_keep_editing: Message,
    on_discard: Message,
    on_save: Message,
) -> Element<'a, Message> {
    container(
        column![
            text("Save changes?").size(16.5).color(theme::TEXT_MAIN),
            text("You changed this button's settings without saving. Save your changes before closing, or discard them?")
                .size(13.0)
                .color(theme::TEXT_DIM),
            rule::horizontal(1).style(theme::divider_rule),
            row![
                button(text("Back").size(13.0))
                    .padding([8.0, 14.0])
                    .on_press(on_keep_editing)
                    .style(theme::btn_secondary),
                space::horizontal(),
                button(text("Discard").size(13.0))
                    .padding([8.0, 14.0])
                    .on_press(on_discard)
                    .style(theme::btn_danger),
                button(text("Save").size(13.0))
                    .padding([8.0, 16.0])
                    .on_press(on_save)
                    .style(theme::btn_primary),
            ]
            .spacing(8.0)
            .align_y(Alignment::Center),
        ]
        .spacing(14.0),
    )
    .width(Length::Fixed(400.0))
    .padding(Padding::new(18.0))
    .style(theme::modal_container)
    .into()
}
