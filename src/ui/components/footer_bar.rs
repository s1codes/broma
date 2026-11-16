// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Bottom footer bar:
//! 1. Microphone selector (passed straight through to the Virtual Mic / Cable by default)
//! 2. Output / Virtual Mic selector (Broma Virtual Mic on Linux, VB-Cable / VoiceMeeter on Windows)
//! 3. Safe Grid layout length controls (`-` / `+` rows within clamped bounds)
//! 4. "Keep in Tray" background mode toggle (enabled by default)
//! 5. Global Volume slider (applies to any button without a custom per-button volume)

use iced::widget::{
    button, checkbox, column, container, pick_list, row, rule, slider, space, text,
};
use iced::{Alignment, Element, Length, Padding};

use crate::ui::theme;

pub struct FooterBarProps<'a, Message> {
    pub microphones: &'a [String],
    pub selected_microphone: Option<&'a String>,

    pub outputs: &'a [String],
    pub selected_output: Option<&'a String>,

    pub grid_label: &'a str,
    pub can_shrink_grid: bool,
    pub can_grow_grid: bool,

    pub close_to_tray: bool,

    pub global_volume: u8,
    pub global_volume_label: &'a str,

    pub on_select_microphone: fn(String) -> Message,
    pub on_select_output: fn(String) -> Message,
    pub on_shrink_grid: Message,
    pub on_grow_grid: Message,
    pub on_toggle_close_to_tray: fn(bool) -> Message,
    pub on_global_volume_change: fn(u8) -> Message,
    pub on_global_volume_commit: Message,
    pub on_stop_all: Message,
    pub on_open_settings: Message,
}

pub fn footer_bar<'a, Message: Clone + 'a>(
    props: FooterBarProps<'a, Message>,
) -> Element<'a, Message> {
    // 1. Microphone (routed through virtual output by default)
    let mic_group = column![
        text("Microphone").size(12.0).color(theme::TEXT_DIM),
        pick_list(
            props.microphones,
            props.selected_microphone,
            props.on_select_microphone,
        )
        .placeholder("Select microphone...")
        .text_size(13.0)
        .padding([6.0, 10.0])
        .width(Length::Fill)
        .style(theme::pick_list_style)
        .menu_style(theme::pick_list_menu_style),
    ]
    .spacing(4.0)
    .width(Length::FillPortion(1));

    // 2. Virtual Mic / Cable Output
    let output_group = column![
        text("Output (Virtual Mic / VB-Cable)")
            .size(12.0)
            .color(theme::TEXT_DIM),
        pick_list(props.outputs, props.selected_output, props.on_select_output)
            .placeholder("Select virtual output...")
            .text_size(13.0)
            .padding([6.0, 10.0])
            .width(Length::Fill)
            .style(theme::pick_list_style)
            .menu_style(theme::pick_list_menu_style),
    ]
    .spacing(4.0)
    .width(Length::FillPortion(1));

    let devices_row = row![mic_group, output_group]
        .spacing(14.0)
        .width(Length::Fill)
        .align_y(Alignment::Center);

    // 3. Grid Layout Length
    let mut shrink_btn = button(text(" - ").size(12.5))
        .padding([5.0, 8.0])
        .style(theme::btn_secondary);
    if props.can_shrink_grid {
        shrink_btn = shrink_btn.on_press(props.on_shrink_grid);
    }

    let mut grow_btn = button(text(" + ").size(12.5))
        .padding([5.0, 8.0])
        .style(theme::btn_secondary);
    if props.can_grow_grid {
        grow_btn = grow_btn.on_press(props.on_grow_grid);
    }

    let grid_length_group = row![
        text("Grid:").size(12.0).color(theme::TEXT_DIM),
        shrink_btn,
        container(text(props.grid_label).size(12.0).color(theme::TEXT_MAIN))
            .padding([4.0, 7.0])
            .style(theme::subtle_well_container),
        grow_btn,
    ]
    .spacing(5.0)
    .align_y(Alignment::Center);

    // 4. Keep in Tray Toggle
    let tray_toggle = checkbox(props.close_to_tray)
        .label("Keep in Tray")
        .on_toggle(props.on_toggle_close_to_tray)
        .size(15.0)
        .text_size(12.0)
        .spacing(5.0)
        .style(theme::checkbox_style);

    // 5. Global Volume Slider
    let global_vol_group = row![
        text("Global Vol:").size(12.0).color(theme::TEXT_DIM),
        container(
            slider(0..=100, props.global_volume, props.on_global_volume_change)
                .default(80)
                .on_release(props.on_global_volume_commit)
                .style(theme::slider_style),
        )
        .width(Length::Fixed(120.0)),
        container(
            text(props.global_volume_label)
                .size(12.0)
                .color(theme::TEXT_MAIN),
        )
        .width(Length::Fixed(34.0)),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);

    let stop_all_btn = button(text("Stop All").size(12.0))
        .padding([5.0, 9.0])
        .on_press(props.on_stop_all)
        .style(theme::btn_secondary);

    let settings_btn = button(text("Settings").size(12.0))
        .padding([5.0, 9.0])
        .on_press(props.on_open_settings)
        .style(theme::btn_secondary);

    let bottom_row = row![
        grid_length_group,
        space::horizontal(),
        tray_toggle,
        global_vol_group,
        stop_all_btn,
        settings_btn,
    ]
    .spacing(8.0)
    .width(Length::Fill)
    .align_y(Alignment::Center);

    container(column![
        rule::horizontal(1).style(theme::divider_rule),
        container(column![devices_row, bottom_row].spacing(10.0))
            .width(Length::Fill)
            .padding(Padding {
                top: 12.0,
                bottom: 12.0,
                left: 16.0,
                right: 16.0,
            }),
    ])
    .width(Length::Fill)
    .style(theme::footer_container)
    .into()
}
