// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Minimal dark-only theme and static widget style functions.
//!
//! Every style function here is a plain `fn` pointer with zero captured state.
//! In Rust, boxing a zero-sized `fn` item does not allocate on the heap, which
//! eliminates per-frame allocator churn during redraws.

use std::sync::LazyLock;

use iced::border;
use iced::theme::Palette;
use iced::widget::{
    button, checkbox, container, overlay::menu, pick_list, rule, scrollable, slider, text_input,
};
use iced::{Background, Border, Color, Shadow, Theme, color};

pub const BG_MAIN: Color = color!(0x121316);
pub const BG_PANEL: Color = color!(0x191B20);
pub const BG_CELL: Color = color!(0x20232A);
pub const BG_CELL_HOVER: Color = color!(0x282C35);
pub const BG_CELL_ACTIVE: Color = color!(0x1A2B22);
pub const BG_INPUT: Color = color!(0x14161A);
pub const BG_SCRIM: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.72);

pub const BORDER_DIM: Color = color!(0x2B2F38);
pub const BORDER_LIGHT: Color = color!(0x3D4350);

pub const ACCENT: Color = color!(0x3B82F6);
pub const ACCENT_HOVER: Color = color!(0x5293FA);
pub const LIVE: Color = color!(0x22C55E);
pub const DANGER: Color = color!(0xDC2626);
pub const DANGER_HOVER: Color = color!(0xEF4444);

pub const TEXT_MAIN: Color = color!(0xECEDEF);
pub const TEXT_DIM: Color = color!(0x9499A3);
pub const TEXT_MUTED: Color = color!(0x636873);

pub const RADIUS: f32 = 6.0;

static DARK_THEME: LazyLock<Theme> = LazyLock::new(|| {
    Theme::custom(
        "Broma Dark",
        Palette {
            background: BG_MAIN,
            text: TEXT_MAIN,
            primary: ACCENT,
            success: LIVE,
            warning: color!(0xEAB308),
            danger: DANGER,
        },
    )
});

pub fn app_theme() -> Theme {
    DARK_THEME.clone()
}

pub fn main_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_MAIN)),
        text_color: Some(TEXT_MAIN),
        ..container::Style::default()
    }
}

pub fn footer_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_PANEL)),
        text_color: Some(TEXT_MAIN),
        ..container::Style::default()
    }
}

pub fn pad_card_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CELL)),
        text_color: Some(TEXT_MAIN),
        border: Border {
            color: BORDER_DIM,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}

pub fn pad_card_playing_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CELL_ACTIVE)),
        text_color: Some(TEXT_MAIN),
        border: Border {
            color: LIVE,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}

pub fn modal_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_PANEL)),
        text_color: Some(TEXT_MAIN),
        border: Border {
            color: BORDER_LIGHT,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}

pub fn scrim_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SCRIM)),
        ..container::Style::default()
    }
}

pub fn subtle_well_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_INPUT)),
        text_color: Some(TEXT_MAIN),
        border: Border {
            color: BORDER_DIM,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}

pub fn divider_rule(_theme: &Theme) -> rule::Style {
    rule::Style {
        color: BORDER_DIM,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

pub fn pad_empty_button(_theme: &Theme, status: button::Status) -> button::Style {
    let (bg, border_color, fg) = match status {
        button::Status::Hovered | button::Status::Pressed => {
            (BG_CELL_HOVER, BORDER_LIGHT, TEXT_MAIN)
        }
        _ => (BG_PANEL, BORDER_DIM, TEXT_DIM),
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

pub fn pad_surface_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => Color::from_rgba(1.0, 1.0, 1.0, 0.04),
        _ => Color::TRANSPARENT,
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color: TEXT_MAIN,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

pub fn btn_primary(_theme: &Theme, status: button::Status) -> button::Style {
    if matches!(status, button::Status::Disabled) {
        return disabled_btn();
    }

    let bg = match status {
        button::Status::Hovered => ACCENT_HOVER,
        _ => ACCENT,
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            color: bg,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

pub fn btn_secondary(_theme: &Theme, status: button::Status) -> button::Style {
    if matches!(status, button::Status::Disabled) {
        return disabled_btn();
    }

    let (bg, border_color) = match status {
        button::Status::Hovered | button::Status::Pressed => (BG_CELL_HOVER, BORDER_LIGHT),
        _ => (BG_CELL, BORDER_DIM),
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color: TEXT_MAIN,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

pub fn btn_small_chip(_theme: &Theme, status: button::Status) -> button::Style {
    if matches!(status, button::Status::Disabled) {
        return disabled_btn();
    }

    let (bg, border_color, fg) = match status {
        button::Status::Hovered | button::Status::Pressed => {
            (BG_CELL_HOVER, BORDER_LIGHT, TEXT_MAIN)
        }
        _ => (BG_INPUT, BORDER_DIM, TEXT_DIM),
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 4.0.into(),
        },
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

pub fn btn_danger(_theme: &Theme, status: button::Status) -> button::Style {
    if matches!(status, button::Status::Disabled) {
        return disabled_btn();
    }

    let bg = match status {
        button::Status::Hovered => DANGER_HOVER,
        _ => DANGER,
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            color: bg,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

fn disabled_btn() -> button::Style {
    button::Style {
        background: Some(Background::Color(BG_INPUT)),
        text_color: TEXT_MUTED,
        border: Border {
            color: BORDER_DIM,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

pub fn text_input_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused { .. } => ACCENT,
        text_input::Status::Hovered => BORDER_LIGHT,
        _ => BORDER_DIM,
    };

    text_input::Style {
        background: Background::Color(BG_INPUT),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: RADIUS.into(),
        },
        icon: TEXT_DIM,
        placeholder: TEXT_MUTED,
        value: TEXT_MAIN,
        selection: Color::from_rgba(0.23, 0.51, 0.96, 0.35),
    }
}

pub fn slider_style(_theme: &Theme, status: slider::Status) -> slider::Style {
    let handle_border = match status {
        slider::Status::Hovered | slider::Status::Dragged => TEXT_MAIN,
        slider::Status::Active => ACCENT,
    };

    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::Color(ACCENT), Background::Color(BORDER_LIGHT)),
            width: 4.0,
            border: border::rounded(2.0),
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 6.0 },
            background: Background::Color(BG_INPUT),
            border_width: 2.0,
            border_color: handle_border,
        },
    }
}

pub fn pick_list_style(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let border_color = match status {
        pick_list::Status::Opened { .. } => ACCENT,
        pick_list::Status::Hovered => BORDER_LIGHT,
        pick_list::Status::Active => BORDER_DIM,
    };

    pick_list::Style {
        text_color: TEXT_MAIN,
        placeholder_color: TEXT_MUTED,
        handle_color: TEXT_DIM,
        background: Background::Color(BG_INPUT),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: RADIUS.into(),
        },
    }
}

pub fn pick_list_menu_style(_theme: &Theme) -> menu::Style {
    menu::Style {
        background: Background::Color(BG_CELL),
        border: Border {
            color: BORDER_LIGHT,
            width: 1.0,
            radius: RADIUS.into(),
        },
        text_color: TEXT_MAIN,
        selected_text_color: Color::WHITE,
        selected_background: Background::Color(ACCENT),
        shadow: Shadow::default(),
    }
}

pub fn scrollable_style(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let mut base = scrollable::default(theme, status);
    let scroller_color = match status {
        scrollable::Status::Active { .. } => BORDER_DIM,
        scrollable::Status::Hovered { .. } | scrollable::Status::Dragged { .. } => BORDER_LIGHT,
    };

    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(scroller_color),
            border: border::rounded(3.0),
        },
    };

    base.container = container::Style::default();
    base.vertical_rail = rail;
    base.horizontal_rail = rail;
    base.gap = None;
    base
}

pub fn checkbox_style(_theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let (is_checked, hovered) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false),
        checkbox::Status::Hovered { is_checked } => (is_checked, true),
        checkbox::Status::Disabled { is_checked } => (is_checked, false),
    };

    let (bg, border_color) = if is_checked {
        let accent = if hovered { ACCENT_HOVER } else { ACCENT };
        (accent, accent)
    } else if hovered {
        (BG_CELL_HOVER, BORDER_LIGHT)
    } else {
        (BG_INPUT, BORDER_DIM)
    };

    checkbox::Style {
        background: Background::Color(bg),
        icon_color: Color::WHITE,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 4.0.into(),
        },
        text_color: Some(if hovered { TEXT_MAIN } else { TEXT_DIM }),
    }
}
