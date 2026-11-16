// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! System tray lifecycle, placeholder icon, context menu, and event routing.
//!
//! Tied directly to the user's `close_to_tray` preference:
//! - When enabled (default), registers the OS tray icon and binds its click and
//!   menu events directly to [`DaemonSender`] and [`AudioHandle`].
//! - When disabled in the footer UI, drops the OS tray icon and any associated
//!   watcher resources immediately so nothing runs needlessly in the background.

pub mod platform;

use std::sync::{Mutex, OnceLock};

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconEvent};

use crate::audio::service::AudioHandle;
use crate::daemon_bus::{DaemonEvent, DaemonSender};

const MENU_TOGGLE_ID: &str = "broma_toggle_window";
const MENU_STOP_ALL_ID: &str = "broma_stop_all";
const MENU_QUIT_ID: &str = "broma_quit";

struct TrayCallbackTarget {
    audio: AudioHandle,
    bus: DaemonSender,
}

static TRAY_TARGET: OnceLock<Mutex<Option<TrayCallbackTarget>>> = OnceLock::new();

/// Placeholder `32x32` solid blue (`#3B82F6`) icon.
/// Swap this single function's bytes when adding custom app icon artwork later.
pub fn default_icon_rgba() -> (Vec<u8>, u32, u32) {
    const WIDTH: u32 = 32;
    const HEIGHT: u32 = 32;
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for _ in 0..(WIDTH * HEIGHT) {
        rgba.extend_from_slice(&[0x3B, 0x82, 0xF6, 0xFF]);
    }
    (rgba, WIDTH, HEIGHT)
}

pub struct TrayController {
    enabled: bool,
    active_icon: Option<TrayIcon>,
}

impl TrayController {
    pub fn new(enabled: bool, audio: AudioHandle, bus: DaemonSender) -> Self {
        install_tray_handlers(audio, bus);
        let mut controller = Self {
            enabled: false,
            active_icon: None,
        };
        controller.set_enabled(enabled);
        controller
    }

    /// Enables or tears down the system tray icon immediately at runtime.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.active_icon = None;
            return;
        }
        if self.active_icon.is_some() {
            return;
        }
        if let Ok((icon, menu)) = build_tray_icon_and_menu() {
            self.active_icon = platform::build_platform_tray(icon, menu).ok();
        }
    }

    /// Returns `true` only when the user opted into close-to-tray AND a live
    /// desktop tray host actually accepted the tray icon.
    pub fn should_close_to_tray(&self) -> bool {
        self.enabled && self.active_icon.is_some()
    }

    pub fn shutdown(&mut self) {
        self.active_icon = None;
        if let Some(cell) = TRAY_TARGET.get()
            && let Ok(mut guard) = cell.lock()
        {
            *guard = None;
        }
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn install_tray_handlers(audio: AudioHandle, bus: DaemonSender) {
    let cell = TRAY_TARGET.get_or_init(|| {
        TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
            let is_left_activate = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            );
            if is_left_activate
                && let Some(lock) = TRAY_TARGET.get()
                && let Ok(guard) = lock.lock()
                && let Some(target) = guard.as_ref()
            {
                target.bus.send(DaemonEvent::TrayToggleWindow);
            }
        }));

        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            let Some(lock) = TRAY_TARGET.get() else {
                return;
            };
            let Ok(guard) = lock.lock() else {
                return;
            };
            let Some(target) = guard.as_ref() else {
                return;
            };

            match event.id().as_ref() {
                MENU_TOGGLE_ID => target.bus.send(DaemonEvent::TrayToggleWindow),
                MENU_STOP_ALL_ID => target.audio.stop_all(),
                MENU_QUIT_ID => target.bus.send(DaemonEvent::TrayQuit),
                _ => {}
            }
        }));

        Mutex::new(None)
    });

    if let Ok(mut guard) = cell.lock() {
        *guard = Some(TrayCallbackTarget { audio, bus });
    }
}

fn build_tray_icon_and_menu() -> Result<(Icon, Menu), String> {
    let (rgba, width, height) = default_icon_rgba();
    let icon = Icon::from_rgba(rgba, width, height)
        .map_err(|e| format!("Failed to construct tray icon: {e}"))?;

    let menu = Menu::new();
    let toggle_item =
        MenuItem::with_id(MenuId::new(MENU_TOGGLE_ID), "Show / Hide Broma", true, None);
    let stop_item = MenuItem::with_id(MenuId::new(MENU_STOP_ALL_ID), "Stop All Sounds", true, None);
    let sep = PredefinedMenuItem::separator();
    let quit_item = MenuItem::with_id(MenuId::new(MENU_QUIT_ID), "Quit Broma", true, None);

    menu.append_items(&[&toggle_item, &stop_item, &sep, &quit_item])
        .map_err(|e| format!("Failed to build tray menu: {e}"))?;

    Ok((icon, menu))
}
