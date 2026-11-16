// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Pure-Wayland Linux system tray via KSNI (`org.kde.StatusNotifierItem` over D-Bus).
//!
//! Because `ksni` sets `assume_sni_available(true)` internally, we first verify
//! that a desktop bar (`org.kde.StatusNotifierWatcher`) actually owns its bus
//! name on D-Bus so barless Wayland sessions never hide the window into nowhere.

use ashpd::zbus;
use iced::futures::executor::block_on;
use tray_icon::menu::Menu;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub fn build_platform_tray(icon: Icon, menu: Menu) -> Result<TrayIcon, String> {
    if !is_status_notifier_watcher_present() {
        return Err(
            "No StatusNotifierWatcher tray host running on this Wayland session".to_string(),
        );
    }

    TrayIconBuilder::new()
        .with_id("io.github.s1codes.broma")
        .with_title("Broma")
        .with_tooltip("Broma Soundboard")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()
        .map_err(|e| format!("Failed to register Linux KSNI tray icon: {e}"))
}

fn is_status_notifier_watcher_present() -> bool {
    block_on(async {
        let Ok(conn) = zbus::Connection::session().await else {
            return false;
        };
        let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else {
            return false;
        };

        if let Ok(name) = "org.kde.StatusNotifierWatcher".try_into()
            && dbus.name_has_owner(name).await.unwrap_or(false)
        {
            return true;
        }
        if let Ok(name) = "org.freedesktop.StatusNotifierWatcher".try_into()
            && dbus.name_has_owner(name).await.unwrap_or(false)
        {
            return true;
        }
        false
    })
}
