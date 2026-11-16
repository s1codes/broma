// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Portable system tray fallback for future OS targets (e.g. macOS).

use tray_icon::menu::Menu;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub fn build_platform_tray(icon: Icon, menu: Menu) -> Result<TrayIcon, String> {
    TrayIconBuilder::new()
        .with_tooltip("Broma Soundboard")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| format!("Failed to register tray icon: {e}"))
}
