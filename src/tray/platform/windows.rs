// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Windows-specific system tray via Win32 `Shell_NotifyIconW`.
//!
//! Explicitly sets `.with_menu_on_left_click(false)` because `tray-icon`
//! defaults to opening the context menu on left-click on Windows, whereas we
//! want left-click to restore/toggle the Broma window and right-click to open the menu.

use tray_icon::menu::Menu;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub fn build_platform_tray(icon: Icon, menu: Menu) -> Result<TrayIcon, String> {
    TrayIconBuilder::new()
        .with_tooltip("Broma Soundboard")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| format!("Failed to register Windows tray icon: {e}"))
}
