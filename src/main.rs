// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod daemon_bus;
mod hotkeys;
#[cfg(target_os = "windows")]
mod single_instance;
mod state;
mod storage;
mod tray;
mod ui;

use iced::{Size, window};
use state::App;

const APP_TITLE: &str = "Broma";

fn main() -> iced::Result {
    #[cfg(target_os = "windows")]
    let _single_instance = match single_instance::acquire() {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(()),
        Err(error) => {
            eprintln!("Failed to establish Broma's single-instance guard: {error}");
            std::process::exit(1);
        }
    };

    iced::daemon(App::boot, App::update, App::view)
        .title(|_state: &App, _window: window::Id| APP_TITLE.to_string())
        .theme(|_state: &App, _window: window::Id| ui::theme::app_theme())
        .subscription(App::subscription)
        .antialiasing(false)
        .run()
}

pub fn main_window_settings() -> window::Settings {
    let (rgba, width, height) = tray::default_icon_rgba();
    let icon = window::icon::from_rgba(rgba, width, height).ok();

    window::Settings {
        size: Size::new(820.0, 620.0),
        min_size: Some(Size::new(660.0, 480.0)),
        position: window::Position::Centered,
        icon,
        exit_on_close_request: false,
        platform_specific: platform_window_settings(),
        ..window::Settings::default()
    }
}

#[cfg(target_os = "linux")]
fn platform_window_settings() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific {
        application_id: hotkeys::platform::linux::WAYLAND_APP_ID.to_string(),
        ..window::settings::PlatformSpecific::default()
    }
}

#[cfg(not(target_os = "linux"))]
fn platform_window_settings() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific::default()
}

#[cfg(test)]
#[path = "../tests/mod.rs"]
mod tests;
