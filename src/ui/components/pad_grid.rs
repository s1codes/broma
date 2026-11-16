// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Scrollable grid container and configurable soundboard pad buttons.
//!
//! Uses a fixed row height inside a `Length::Fill` `scrollable` viewport so
//! increasing the grid length scrolls cleanly without expanding the window or
//! pushing the footer off-screen.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, mouse_area, row, scrollable, space, text};
use iced::{Alignment, Element, Length, Padding};

use crate::storage::GRID_COLS;
use crate::ui::theme;

const PAD_ROW_HEIGHT: f32 = 122.0;

/// Pre-formatted view state for one slot so `view()` performs zero `format!()` allocations.
#[derive(Debug, Clone)]
pub struct PadViewItem {
    pub is_empty: bool,
    pub has_file: bool,
    pub title: String,
    pub meta_label: String,
    pub keybind_label: String,
}

pub fn pad_grid<'a, Message: Clone + 'a>(
    items: &'a [PadViewItem],
    active_slots_mask: u64,
    on_pad_click: fn(usize) -> Message,
    on_open_config: fn(usize) -> Message,
) -> Element<'a, Message> {
    let mut rows_col = column![].spacing(10.0).width(Length::Fill);

    for (chunk_idx, row_slice) in items.chunks(GRID_COLS).enumerate() {
        let base_idx = chunk_idx * GRID_COLS;
        let mut grid_row = row![]
            .spacing(10.0)
            .width(Length::Fill)
            .height(Length::Fixed(PAD_ROW_HEIGHT));

        for (offset, item) in row_slice.iter().enumerate() {
            let slot_idx = base_idx + offset;
            let is_playing = slot_idx < 64 && (active_slots_mask & (1u64 << slot_idx)) != 0;
            grid_row = grid_row.push(pad_cell(
                slot_idx,
                item,
                is_playing,
                on_pad_click,
                on_open_config,
            ));
        }

        // Pad any partial row safely with invisible spacers to preserve 3-column geometry.
        for _ in row_slice.len()..GRID_COLS {
            grid_row = grid_row.push(space().width(Length::FillPortion(1)));
        }

        rows_col = rows_col.push(grid_row);
    }

    let scroll_area = scrollable(container(rows_col).width(Length::Fill).padding(Padding {
        top: 14.0,
        bottom: 14.0,
        left: 16.0,
        right: 16.0,
    }))
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::scrollable_style);

    container(scroll_area)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn pad_cell<'a, Message: Clone + 'a>(
    slot_idx: usize,
    item: &'a PadViewItem,
    is_playing: bool,
    on_pad_click: fn(usize) -> Message,
    on_open_config: fn(usize) -> Message,
) -> Element<'a, Message> {
    if item.is_empty {
        let empty_content = column![
            text("Empty").size(14.0).color(theme::TEXT_DIM),
            text("Click to configure")
                .size(12.0)
                .color(theme::TEXT_MUTED),
        ]
        .spacing(4.0)
        .align_x(Alignment::Center);

        return button(
            container(empty_content)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .width(Length::FillPortion(1))
        .height(Length::Fill)
        .on_press(on_open_config(slot_idx))
        .style(theme::pad_empty_button)
        .into();
    }

    let top_strip = row![
        text(&item.keybind_label).size(11.5).color(if is_playing {
            theme::LIVE
        } else {
            theme::TEXT_MUTED
        }),
        space::horizontal(),
        button(text("Configure").size(11.5))
            .padding([3.0, 8.0])
            .on_press(on_open_config(slot_idx))
            .style(theme::btn_small_chip),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let main_action = if item.has_file {
        on_pad_click(slot_idx)
    } else {
        on_open_config(slot_idx)
    };

    let body_trigger = button(
        column![
            space::vertical(),
            text(&item.title).size(15.0).color(theme::TEXT_MAIN),
            text(if is_playing {
                "Playing..."
            } else {
                &item.meta_label
            })
            .size(11.5)
            .color(if is_playing {
                theme::LIVE
            } else {
                theme::TEXT_DIM
            }),
        ]
        .spacing(4.0)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding([4.0, 4.0])
    .on_press(main_action)
    .style(theme::pad_surface_button);

    let card = container(
        column![top_strip, body_trigger]
            .spacing(4.0)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(10.0)
    .style(if is_playing {
        theme::pad_card_playing_container
    } else {
        theme::pad_card_container
    });

    mouse_area(card)
        .on_right_press(on_open_config(slot_idx))
        .into()
}
