// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Settings & Library modal:
//! - Local Monitor Output (Headphones) selector
//! - Independent Monitor Volume & Virtual Mic / Cable Soundboard Volume sliders
//! - Refresh Audio Devices trigger
//! - Library disk usage display + `.bromapack` backup export/import
//! - Danger Zone "Delete All Data" with two-step confirmation

use iced::widget::{
    button, column, container, mouse_area, opaque, pick_list, row, rule, slider, space, text,
};
use iced::{Alignment, Element, Length, Padding};

use crate::storage::DEFAULT_STREAM_VOLUME;
use crate::ui::theme;

pub struct SettingsModalProps<'a, Message> {
    pub monitor_outputs: &'a [String],
    pub selected_monitor: Option<&'a String>,

    pub monitor_volume: u8,
    pub monitor_volume_label: &'a str,

    pub cable_volume: u8,
    pub cable_volume_label: &'a str,

    pub library_stats_label: &'a str,
    pub status_label: &'a str,
    pub has_error: bool,
    pub show_delete_confirm: bool,

    pub on_backdrop_click: Message,
    pub on_refresh_devices: Message,
    pub on_select_monitor: fn(String) -> Message,
    pub on_monitor_volume_change: fn(u8) -> Message,
    pub on_cable_volume_change: fn(u8) -> Message,
    pub on_commit_stream_volumes: Message,
    pub on_export_backup: Message,
    pub on_import_backup: Message,
    pub on_request_delete_all: Message,
    pub on_cancel_delete_all: Message,
    pub on_confirm_delete_all: Message,
    pub on_close: Message,
}

pub fn settings_modal<'a, Message: Clone + 'a>(
    props: SettingsModalProps<'a, Message>,
) -> Element<'a, Message> {
    let backdrop_msg = props.on_backdrop_click.clone();

    let dialog_content: Element<'a, Message> = if props.show_delete_confirm {
        delete_confirm_card(props.on_cancel_delete_all, props.on_confirm_delete_all)
    } else {
        settings_card(props)
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

fn settings_card<'a, Message: Clone + 'a>(
    props: SettingsModalProps<'a, Message>,
) -> Element<'a, Message> {
    let header = row![
        text("Settings & Library")
            .size(16.0)
            .color(theme::TEXT_MAIN),
        space::horizontal(),
        button(text("Refresh Devices").size(11.5))
            .padding([4.0, 9.0])
            .on_press(props.on_refresh_devices)
            .style(theme::btn_small_chip),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);

    // 1. Monitor Output (Headphones)
    let monitor_section = column![
        text("Monitor Output (Headphones)")
            .size(12.5)
            .color(theme::TEXT_DIM),
        pick_list(
            props.monitor_outputs,
            props.selected_monitor,
            props.on_select_monitor,
        )
        .placeholder("Select monitor output...")
        .text_size(13.0)
        .padding([7.0, 10.0])
        .width(Length::Fill)
        .style(theme::pick_list_style)
        .menu_style(theme::pick_list_menu_style),
    ]
    .spacing(5.0);

    // 2. Independent Stream Volumes
    let monitor_vol_section = column![
        row![
            text("Monitor Volume").size(12.5).color(theme::TEXT_DIM),
            space::horizontal(),
            text(props.monitor_volume_label)
                .size(12.0)
                .color(theme::TEXT_MAIN),
        ]
        .align_y(Alignment::Center),
        slider(
            0..=100,
            props.monitor_volume,
            props.on_monitor_volume_change
        )
        .default(DEFAULT_STREAM_VOLUME)
        .on_release(props.on_commit_stream_volumes.clone())
        .style(theme::slider_style),
    ]
    .spacing(5.0);

    let cable_vol_section = column![
        row![
            text("Virtual Mic / Cable Volume")
                .size(12.5)
                .color(theme::TEXT_DIM),
            space::horizontal(),
            text(props.cable_volume_label)
                .size(12.0)
                .color(theme::TEXT_MAIN),
        ]
        .align_y(Alignment::Center),
        slider(0..=100, props.cable_volume, props.on_cable_volume_change)
            .default(DEFAULT_STREAM_VOLUME)
            .on_release(props.on_commit_stream_volumes)
            .style(theme::slider_style),
    ]
    .spacing(5.0);

    // 3. Library Storage & Backup
    let mut storage_col = column![
        text("Library Storage").size(12.5).color(theme::TEXT_DIM),
        container(
            text(props.library_stats_label)
                .size(12.5)
                .color(theme::TEXT_MAIN),
        )
        .width(Length::Fill)
        .padding([8.0, 10.0])
        .style(theme::subtle_well_container),
        row![
            button(text("Export Backup...").size(12.5))
                .padding([7.0, 12.0])
                .on_press(props.on_export_backup)
                .style(theme::btn_secondary),
            button(text("Import Backup...").size(12.5))
                .padding([7.0, 12.0])
                .on_press(props.on_import_backup)
                .style(theme::btn_secondary),
        ]
        .spacing(8.0),
    ]
    .spacing(7.0);

    if !props.status_label.is_empty() {
        storage_col = storage_col.push(text(props.status_label).size(12.0).color(
            if props.has_error {
                theme::DANGER_HOVER
            } else {
                theme::LIVE
            },
        ));
    }

    // 4. Danger Zone & Bottom Close
    let bottom_row = row![
        button(text("Delete All Data...").size(12.5))
            .padding([8.0, 12.0])
            .on_press(props.on_request_delete_all)
            .style(theme::btn_danger),
        space::horizontal(),
        button(text("Done").size(13.0))
            .padding([8.0, 18.0])
            .on_press(props.on_close)
            .style(theme::btn_primary),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(
        column![
            header,
            rule::horizontal(1).style(theme::divider_rule),
            monitor_section,
            monitor_vol_section,
            cable_vol_section,
            rule::horizontal(1).style(theme::divider_rule),
            storage_col,
            rule::horizontal(1).style(theme::divider_rule),
            bottom_row,
        ]
        .spacing(12.0),
    )
    .width(Length::Fixed(460.0))
    .padding(Padding::new(18.0))
    .style(theme::modal_container)
    .into()
}

fn delete_confirm_card<'a, Message: Clone + 'a>(
    on_cancel: Message,
    on_confirm: Message,
) -> Element<'a, Message> {
    container(
        column![
            text("Delete all soundboard data?")
                .size(16.5)
                .color(theme::TEXT_MAIN),
            text("This permanently removes every button, keybind, setting, and imported audio file from Broma's local library folder. This cannot be undone.")
                .size(13.0)
                .color(theme::TEXT_DIM),
            rule::horizontal(1).style(theme::divider_rule),
            row![
                button(text("Cancel").size(13.0))
                    .padding([8.0, 16.0])
                    .on_press(on_cancel)
                    .style(theme::btn_secondary),
                space::horizontal(),
                button(text("Delete Everything").size(13.0))
                    .padding([8.0, 16.0])
                    .on_press(on_confirm)
                    .style(theme::btn_danger),
            ]
            .spacing(8.0)
            .align_y(Alignment::Center),
        ]
        .spacing(14.0),
    )
    .width(Length::Fixed(420.0))
    .padding(Padding::new(18.0))
    .style(theme::modal_container)
    .into()
}
