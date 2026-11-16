// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Windows-specific global hotkeys via Win32 `RegisterHotKey` (`global-hotkey`).
//!
//! Dispatches `HotKeyState::Pressed` activations directly to [`AudioHandle`]
//! inside the Win32 `WM_HOTKEY` callback—zero `iced::Subscription` round-trips,
//! zero low-level keyboard hooks (`WH_KEYBOARD_LL`), and zero polling timers.
//!
//! Windows-specific safeguards:
//! - Calling `set_suspended(true)` while recording a new keybind in the Configure
//!   Modal temporarily unregisters active Win32 grabs so `WM_HOTKEY` does not
//!   swallow keystrokes before `winit`/`iced` receives `WM_KEYDOWN`.
//! - Tracks per-slot registration errors (`ERROR_HOTKEY_ALREADY_REGISTERED` /
//!   Win32-reserved combinations such as bare `F12` or `Win + G`) so conflicts
//!   are reported clearly instead of failing silently.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use global_hotkey::Error as GlobalHotKeyError;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::audio::service::AudioHandle;
use crate::daemon_bus::{DaemonEvent, DaemonSender};
use crate::hotkeys::{KeyToken, ParsedKeybind, SlotShortcutSpec};
use crate::storage::SlotConfig;

struct Win32DispatchTable {
    audio: AudioHandle,
    id_to_slot: HashMap<u32, usize>,
}

static DISPATCH: OnceLock<Mutex<Option<Win32DispatchTable>>> = OnceLock::new();

pub struct WindowsHotkeyEngine {
    manager: Option<GlobalHotKeyManager>,
    desired_specs: Vec<SlotShortcutSpec>,
    pub(crate) registered: Vec<HotKey>,
    id_to_slot: HashMap<u32, usize>,
    pub(crate) slot_errors: HashMap<usize, String>,
    suspended: bool,
    last_error: Option<String>,
    audio: AudioHandle,
    bus: DaemonSender,
}

impl WindowsHotkeyEngine {
    pub fn new(audio: AudioHandle, bus: DaemonSender) -> Self {
        let cell = DISPATCH.get_or_init(|| {
            GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
                if event.state == HotKeyState::Pressed
                    && let Some(lock) = DISPATCH.get()
                    && let Ok(guard) = lock.lock()
                    && let Some(table) = guard.as_ref()
                    && let Some(&slot_idx) = table.id_to_slot.get(&event.id)
                {
                    table.audio.toggle_slot(slot_idx);
                }
            }));
            Mutex::new(None)
        });

        if let Ok(mut guard) = cell.lock() {
            *guard = Some(Win32DispatchTable {
                audio: audio.clone(),
                id_to_slot: HashMap::new(),
            });
        }

        let (manager, last_error) = match GlobalHotKeyManager::new() {
            Ok(mgr) => (Some(mgr), None),
            Err(e) => {
                let msg = format!("Failed to initialize Windows hotkeys: {e}");
                bus.send(DaemonEvent::HotkeyError(msg.clone()));
                (None, Some(msg))
            }
        };

        Self {
            manager,
            desired_specs: Vec::new(),
            registered: Vec::new(),
            id_to_slot: HashMap::new(),
            slot_errors: HashMap::new(),
            suspended: false,
            last_error,
            audio,
            bus,
        }
    }

    #[cfg(test)]
    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn slot_for_hotkey_id(&self, id: u32) -> Option<usize> {
        if self.suspended {
            None
        } else {
            self.id_to_slot.get(&id).copied()
        }
    }

    pub fn set_suspended(&mut self, suspended: bool) {
        if self.suspended == suspended {
            return;
        }
        self.suspended = suspended;
        if suspended {
            self.unregister_active_grabs();
        } else {
            self.apply_active_grabs();
        }
    }

    pub fn sync_slots(&mut self, slots: &[SlotConfig], visible_count: usize) {
        self.desired_specs = crate::hotkeys::collect_visible_specs(slots, visible_count);
        if self.suspended {
            self.unregister_active_grabs();
        } else {
            self.apply_active_grabs();
        }
    }

    pub fn shutdown(&mut self) {
        self.unregister_active_grabs();
        if let Some(cell) = DISPATCH.get()
            && let Ok(mut guard) = cell.lock()
        {
            *guard = None;
        }
    }

    fn unregister_active_grabs(&mut self) {
        if let Some(manager) = &self.manager
            && !self.registered.is_empty()
        {
            let _ = manager.unregister_all(&self.registered);
        }
        self.registered.clear();
        self.id_to_slot.clear();
        self.publish_dispatch_map();
    }

    fn apply_active_grabs(&mut self) {
        self.unregister_active_grabs();
        self.slot_errors.clear();

        let Some(manager) = &self.manager else {
            return;
        };
        self.last_error = None;

        for spec in &self.desired_specs {
            let formatted = spec.parsed.format_canonical();
            let Some(hk) = to_win32_hotkey(spec.parsed) else {
                let msg = format!("Shortcut '{formatted}' is not supported on Windows");
                self.slot_errors.insert(spec.slot_index, msg.clone());
                if self.last_error.is_none() {
                    self.last_error = Some(msg.clone());
                }
                self.bus.send(DaemonEvent::HotkeyError(msg));
                continue;
            };

            match manager.register(hk) {
                Ok(()) => {
                    self.id_to_slot.insert(hk.id(), spec.slot_index);
                    self.registered.push(hk);
                }
                Err(err) => {
                    let msg = format_win32_register_error(&formatted, &err);
                    self.slot_errors.insert(spec.slot_index, msg.clone());
                    if self.last_error.is_none() {
                        self.last_error = Some(msg.clone());
                    }
                    self.bus.send(DaemonEvent::HotkeyError(msg));
                }
            }
        }

        self.publish_dispatch_map();
    }

    fn publish_dispatch_map(&self) {
        if let Some(cell) = DISPATCH.get()
            && let Ok(mut guard) = cell.lock()
        {
            *guard = Some(Win32DispatchTable {
                audio: self.audio.clone(),
                id_to_slot: self.id_to_slot.clone(),
            });
        }
    }
}

impl Drop for WindowsHotkeyEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn format_win32_register_error(shortcut: &str, err: &GlobalHotKeyError) -> String {
    match err {
        GlobalHotKeyError::AlreadyRegistered(_) => {
            format!("Shortcut '{shortcut}' is already reserved by Windows or another app")
        }
        other => format!("Could not register shortcut '{shortcut}' on Windows: {other}"),
    }
}

pub(crate) fn to_win32_hotkey(kb: ParsedKeybind) -> Option<HotKey> {
    let mut mods = Modifiers::empty();
    if kb.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if kb.alt {
        mods |= Modifiers::ALT;
    }
    if kb.shift {
        mods |= Modifiers::SHIFT;
    }
    if kb.logo {
        mods |= Modifiers::META;
    }

    let code = match kb.key {
        KeyToken::F(1) => Code::F1,
        KeyToken::F(2) => Code::F2,
        KeyToken::F(3) => Code::F3,
        KeyToken::F(4) => Code::F4,
        KeyToken::F(5) => Code::F5,
        KeyToken::F(6) => Code::F6,
        KeyToken::F(7) => Code::F7,
        KeyToken::F(8) => Code::F8,
        KeyToken::F(9) => Code::F9,
        KeyToken::F(10) => Code::F10,
        KeyToken::F(11) => Code::F11,
        KeyToken::F(12) => Code::F12,
        KeyToken::F(13) => Code::F13,
        KeyToken::F(14) => Code::F14,
        KeyToken::F(15) => Code::F15,
        KeyToken::F(16) => Code::F16,
        KeyToken::F(17) => Code::F17,
        KeyToken::F(18) => Code::F18,
        KeyToken::F(19) => Code::F19,
        KeyToken::F(20) => Code::F20,
        KeyToken::F(21) => Code::F21,
        KeyToken::F(22) => Code::F22,
        KeyToken::F(23) => Code::F23,
        KeyToken::F(24) => Code::F24,
        KeyToken::Letter('A') => Code::KeyA,
        KeyToken::Letter('B') => Code::KeyB,
        KeyToken::Letter('C') => Code::KeyC,
        KeyToken::Letter('D') => Code::KeyD,
        KeyToken::Letter('E') => Code::KeyE,
        KeyToken::Letter('F') => Code::KeyF,
        KeyToken::Letter('G') => Code::KeyG,
        KeyToken::Letter('H') => Code::KeyH,
        KeyToken::Letter('I') => Code::KeyI,
        KeyToken::Letter('J') => Code::KeyJ,
        KeyToken::Letter('K') => Code::KeyK,
        KeyToken::Letter('L') => Code::KeyL,
        KeyToken::Letter('M') => Code::KeyM,
        KeyToken::Letter('N') => Code::KeyN,
        KeyToken::Letter('O') => Code::KeyO,
        KeyToken::Letter('P') => Code::KeyP,
        KeyToken::Letter('Q') => Code::KeyQ,
        KeyToken::Letter('R') => Code::KeyR,
        KeyToken::Letter('S') => Code::KeyS,
        KeyToken::Letter('T') => Code::KeyT,
        KeyToken::Letter('U') => Code::KeyU,
        KeyToken::Letter('V') => Code::KeyV,
        KeyToken::Letter('W') => Code::KeyW,
        KeyToken::Letter('X') => Code::KeyX,
        KeyToken::Letter('Y') => Code::KeyY,
        KeyToken::Letter('Z') => Code::KeyZ,
        KeyToken::Digit(0) => Code::Digit0,
        KeyToken::Digit(1) => Code::Digit1,
        KeyToken::Digit(2) => Code::Digit2,
        KeyToken::Digit(3) => Code::Digit3,
        KeyToken::Digit(4) => Code::Digit4,
        KeyToken::Digit(5) => Code::Digit5,
        KeyToken::Digit(6) => Code::Digit6,
        KeyToken::Digit(7) => Code::Digit7,
        KeyToken::Digit(8) => Code::Digit8,
        KeyToken::Digit(9) => Code::Digit9,
        KeyToken::NumpadDigit(0) => Code::Numpad0,
        KeyToken::NumpadDigit(1) => Code::Numpad1,
        KeyToken::NumpadDigit(2) => Code::Numpad2,
        KeyToken::NumpadDigit(3) => Code::Numpad3,
        KeyToken::NumpadDigit(4) => Code::Numpad4,
        KeyToken::NumpadDigit(5) => Code::Numpad5,
        KeyToken::NumpadDigit(6) => Code::Numpad6,
        KeyToken::NumpadDigit(7) => Code::Numpad7,
        KeyToken::NumpadDigit(8) => Code::Numpad8,
        KeyToken::NumpadDigit(9) => Code::Numpad9,
        KeyToken::NumpadAdd => Code::NumpadAdd,
        KeyToken::NumpadSubtract => Code::NumpadSubtract,
        KeyToken::NumpadMultiply => Code::NumpadMultiply,
        KeyToken::NumpadDivide => Code::NumpadDivide,
        KeyToken::NumpadDecimal => Code::NumpadDecimal,
        KeyToken::NumpadEnter => Code::NumpadEnter,
        KeyToken::Space => Code::Space,
        KeyToken::Tab => Code::Tab,
        KeyToken::Enter => Code::Enter,
        KeyToken::Backspace => Code::Backspace,
        KeyToken::Delete => Code::Delete,
        KeyToken::Insert => Code::Insert,
        KeyToken::Home => Code::Home,
        KeyToken::End => Code::End,
        KeyToken::PageUp => Code::PageUp,
        KeyToken::PageDown => Code::PageDown,
        KeyToken::Up => Code::ArrowUp,
        KeyToken::Down => Code::ArrowDown,
        KeyToken::Left => Code::ArrowLeft,
        KeyToken::Right => Code::ArrowRight,
        KeyToken::Backquote => Code::Backquote,
        KeyToken::Minus => Code::Minus,
        KeyToken::Equal => Code::Equal,
        KeyToken::BracketLeft => Code::BracketLeft,
        KeyToken::BracketRight => Code::BracketRight,
        KeyToken::Backslash => Code::Backslash,
        KeyToken::Semicolon => Code::Semicolon,
        KeyToken::Quote => Code::Quote,
        KeyToken::Comma => Code::Comma,
        KeyToken::Period => Code::Period,
        KeyToken::Slash => Code::Slash,
        _ => return None,
    };

    let mods_opt = (!mods.is_empty()).then_some(mods);
    Some(HotKey::new(mods_opt, code))
}
