// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Global hotkey capture, canonical keybind parsing, duplicate-slot resolution,
//! and OS-specific registration (`src/hotkeys/platform/`).
//!
//! Hotkey activations dispatch directly to [`crate::audio::service::AudioHandle`]
//! without bouncing through the GUI state machine, keeping hotkey-to-sound
//! latency minimal even when the UI window is closed to tray.

pub mod platform;

use iced::keyboard::key::{Code as IcedCode, Named, Physical};
use iced::keyboard::{Key, Modifiers as IcedModifiers};

use crate::storage::SlotConfig;

pub use platform::PlatformHotkeyEngine;

/// Normalized shortcut specification derived from a visible pad [`SlotConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotShortcutSpec {
    pub slot_index: usize,
    pub label: String,
    pub parsed: ParsedKeybind,
}

/// Platform-neutral representation of a single keyboard shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParsedKeybind {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
    pub key: KeyToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyToken {
    F(u8),        // 1..=24
    Letter(char), // 'A'..='Z'
    Digit(u8),    // 0..=9
    NumpadDigit(u8),
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadDecimal,
    NumpadEnter,
    Space,
    Tab,
    Enter,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Backquote,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
}

impl KeyToken {
    pub fn canonical_name(self) -> String {
        match self {
            Self::F(n) => format!("F{n}"),
            Self::Letter(ch) => ch.to_string(),
            Self::Digit(d) => d.to_string(),
            Self::NumpadDigit(d) => format!("Numpad{d}"),
            Self::NumpadAdd => "NumpadAdd".to_string(),
            Self::NumpadSubtract => "NumpadSubtract".to_string(),
            Self::NumpadMultiply => "NumpadMultiply".to_string(),
            Self::NumpadDivide => "NumpadDivide".to_string(),
            Self::NumpadDecimal => "NumpadDecimal".to_string(),
            Self::NumpadEnter => "NumpadEnter".to_string(),
            Self::Space => "Space".to_string(),
            Self::Tab => "Tab".to_string(),
            Self::Enter => "Enter".to_string(),
            Self::Backspace => "Backspace".to_string(),
            Self::Delete => "Delete".to_string(),
            Self::Insert => "Insert".to_string(),
            Self::Home => "Home".to_string(),
            Self::End => "End".to_string(),
            Self::PageUp => "PageUp".to_string(),
            Self::PageDown => "PageDown".to_string(),
            Self::Up => "Up".to_string(),
            Self::Down => "Down".to_string(),
            Self::Left => "Left".to_string(),
            Self::Right => "Right".to_string(),
            Self::Backquote => "`".to_string(),
            Self::Minus => "-".to_string(),
            Self::Equal => "=".to_string(),
            Self::BracketLeft => "[".to_string(),
            Self::BracketRight => "]".to_string(),
            Self::Backslash => "\\".to_string(),
            Self::Semicolon => ";".to_string(),
            Self::Quote => "'".to_string(),
            Self::Comma => ",".to_string(),
            Self::Period => ".".to_string(),
            Self::Slash => "/".to_string(),
        }
    }
}

impl ParsedKeybind {
    pub fn format_canonical(self) -> String {
        let mut out = String::with_capacity(20);
        if self.ctrl {
            out.push_str("Ctrl + ");
        }
        if self.alt {
            out.push_str("Alt + ");
        }
        if self.shift {
            out.push_str("Shift + ");
        }
        if self.logo {
            out.push_str("Super + ");
        }
        out.push_str(&self.key.canonical_name());
        out
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Empty keybind".to_string());
        }

        let parts: Vec<&str> = trimmed.split('+').map(str::trim).collect();
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut logo = false;
        let mut main_key: Option<KeyToken> = None;

        for token in parts {
            if token.is_empty() {
                return Err(format!("Invalid keybind '{raw}'"));
            }
            if main_key.is_some() {
                return Err(format!(
                    "Modifiers must come before the main key in '{raw}'"
                ));
            }
            match token.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => ctrl = true,
                "ALT" | "OPTION" => alt = true,
                "SHIFT" => shift = true,
                "SUPER" | "META" | "WIN" | "CMD" | "COMMAND" | "LOGO" => logo = true,
                _ => {
                    main_key = Some(parse_key_token(token)?);
                }
            }
        }

        let key = main_key.ok_or_else(|| format!("Missing main key in '{raw}'"))?;
        Ok(Self {
            ctrl,
            alt,
            shift,
            logo,
            key,
        })
    }
}

pub fn capture_iced_keybind(
    physical_key: &Physical,
    logical_key: &Key,
    modifiers: IcedModifiers,
) -> Option<String> {
    let key_token = match physical_key {
        Physical::Code(code) => map_iced_code(*code),
        Physical::Unidentified(_) => None,
    }
    .or_else(|| map_logical_fallback(logical_key))?;

    let parsed = ParsedKeybind {
        ctrl: modifiers.control(),
        alt: modifiers.alt(),
        shift: modifiers.shift(),
        logo: modifiers.logo(),
        key: key_token,
    };
    Some(parsed.format_canonical())
}

pub fn find_conflicting_slot(
    slots: &[SlotConfig],
    target_slot: usize,
    candidate_keybind: &str,
) -> Option<usize> {
    let Ok(candidate) = ParsedKeybind::parse(candidate_keybind) else {
        return None;
    };
    slots.iter().enumerate().find_map(|(idx, slot)| {
        if idx == target_slot {
            return None;
        }
        let existing = slot.keybind.as_deref()?;
        let parsed = ParsedKeybind::parse(existing).ok()?;
        (parsed == candidate).then_some(idx)
    })
}

pub fn collect_visible_specs(slots: &[SlotConfig], visible_count: usize) -> Vec<SlotShortcutSpec> {
    let limit = visible_count.min(slots.len());
    let mut specs = Vec::new();

    for (idx, slot) in slots[..limit].iter().enumerate() {
        if slot.stored_file.is_none() {
            continue;
        }
        let Some(kb_str) = slot.keybind.as_deref() else {
            continue;
        };
        let Ok(parsed) = ParsedKeybind::parse(kb_str) else {
            continue;
        };
        let label = if slot.name.trim().is_empty() {
            format!("Soundboard Pad #{}", idx + 1)
        } else {
            format!("Play: {}", slot.name.trim())
        };
        specs.push(SlotShortcutSpec {
            slot_index: idx,
            label,
            parsed,
        });
    }

    specs
}

fn parse_key_token(token: &str) -> Result<KeyToken, String> {
    let upper = token.to_ascii_uppercase();

    if upper.len() == 1 {
        let ch = upper.chars().next().unwrap();
        if ch.is_ascii_uppercase() {
            return Ok(KeyToken::Letter(ch));
        }
        if ch.is_ascii_digit() {
            return Ok(KeyToken::Digit(ch as u8 - b'0'));
        }
    }

    if let Some(rest) = upper.strip_prefix('F')
        && let Ok(num) = rest.parse::<u8>()
        && (1..=24).contains(&num)
    {
        return Ok(KeyToken::F(num));
    }

    if let Some(rest) = upper
        .strip_prefix("NUMPAD")
        .or_else(|| upper.strip_prefix("NUM"))
        && rest.len() == 1
        && let Some(d) = rest.chars().next().filter(char::is_ascii_digit)
    {
        return Ok(KeyToken::NumpadDigit(d as u8 - b'0'));
    }

    match upper.as_str() {
        "NUMPADADD" | "NUMADD" | "NUMPADPLUS" => Ok(KeyToken::NumpadAdd),
        "NUMPADSUBTRACT" | "NUMSUBTRACT" | "NUMPADMINUS" => Ok(KeyToken::NumpadSubtract),
        "NUMPADMULTIPLY" | "NUMMULTIPLY" | "NUMPADSTAR" => Ok(KeyToken::NumpadMultiply),
        "NUMPADDIVIDE" | "NUMDIVIDE" => Ok(KeyToken::NumpadDivide),
        "NUMPADDECIMAL" | "NUMDECIMAL" => Ok(KeyToken::NumpadDecimal),
        "NUMPADENTER" | "NUMENTER" => Ok(KeyToken::NumpadEnter),
        "SPACE" => Ok(KeyToken::Space),
        "TAB" => Ok(KeyToken::Tab),
        "ENTER" | "RETURN" => Ok(KeyToken::Enter),
        "BACKSPACE" => Ok(KeyToken::Backspace),
        "DELETE" | "DEL" => Ok(KeyToken::Delete),
        "INSERT" | "INS" => Ok(KeyToken::Insert),
        "HOME" => Ok(KeyToken::Home),
        "END" => Ok(KeyToken::End),
        "PAGEUP" | "PGUP" => Ok(KeyToken::PageUp),
        "PAGEDOWN" | "PGDN" => Ok(KeyToken::PageDown),
        "UP" | "ARROWUP" => Ok(KeyToken::Up),
        "DOWN" | "ARROWDOWN" => Ok(KeyToken::Down),
        "LEFT" | "ARROWLEFT" => Ok(KeyToken::Left),
        "RIGHT" | "ARROWRIGHT" => Ok(KeyToken::Right),
        "`" | "BACKQUOTE" | "GRAVE" => Ok(KeyToken::Backquote),
        "-" | "MINUS" => Ok(KeyToken::Minus),
        "=" | "EQUAL" => Ok(KeyToken::Equal),
        "[" | "BRACKETLEFT" => Ok(KeyToken::BracketLeft),
        "]" | "BRACKETRIGHT" => Ok(KeyToken::BracketRight),
        "\\" | "BACKSLASH" => Ok(KeyToken::Backslash),
        ";" | "SEMICOLON" => Ok(KeyToken::Semicolon),
        "'" | "QUOTE" | "APOSTROPHE" => Ok(KeyToken::Quote),
        "," | "COMMA" => Ok(KeyToken::Comma),
        "." | "PERIOD" => Ok(KeyToken::Period),
        "/" | "SLASH" => Ok(KeyToken::Slash),
        _ => Err(format!("Unsupported hotkey key '{token}'")),
    }
}

fn map_iced_code(code: IcedCode) -> Option<KeyToken> {
    Some(match code {
        IcedCode::ControlLeft
        | IcedCode::ControlRight
        | IcedCode::ShiftLeft
        | IcedCode::ShiftRight
        | IcedCode::AltLeft
        | IcedCode::AltRight
        | IcedCode::SuperLeft
        | IcedCode::SuperRight => return None,

        IcedCode::F1 => KeyToken::F(1),
        IcedCode::F2 => KeyToken::F(2),
        IcedCode::F3 => KeyToken::F(3),
        IcedCode::F4 => KeyToken::F(4),
        IcedCode::F5 => KeyToken::F(5),
        IcedCode::F6 => KeyToken::F(6),
        IcedCode::F7 => KeyToken::F(7),
        IcedCode::F8 => KeyToken::F(8),
        IcedCode::F9 => KeyToken::F(9),
        IcedCode::F10 => KeyToken::F(10),
        IcedCode::F11 => KeyToken::F(11),
        IcedCode::F12 => KeyToken::F(12),
        IcedCode::F13 => KeyToken::F(13),
        IcedCode::F14 => KeyToken::F(14),
        IcedCode::F15 => KeyToken::F(15),
        IcedCode::F16 => KeyToken::F(16),
        IcedCode::F17 => KeyToken::F(17),
        IcedCode::F18 => KeyToken::F(18),
        IcedCode::F19 => KeyToken::F(19),
        IcedCode::F20 => KeyToken::F(20),
        IcedCode::F21 => KeyToken::F(21),
        IcedCode::F22 => KeyToken::F(22),
        IcedCode::F23 => KeyToken::F(23),
        IcedCode::F24 => KeyToken::F(24),

        IcedCode::KeyA => KeyToken::Letter('A'),
        IcedCode::KeyB => KeyToken::Letter('B'),
        IcedCode::KeyC => KeyToken::Letter('C'),
        IcedCode::KeyD => KeyToken::Letter('D'),
        IcedCode::KeyE => KeyToken::Letter('E'),
        IcedCode::KeyF => KeyToken::Letter('F'),
        IcedCode::KeyG => KeyToken::Letter('G'),
        IcedCode::KeyH => KeyToken::Letter('H'),
        IcedCode::KeyI => KeyToken::Letter('I'),
        IcedCode::KeyJ => KeyToken::Letter('J'),
        IcedCode::KeyK => KeyToken::Letter('K'),
        IcedCode::KeyL => KeyToken::Letter('L'),
        IcedCode::KeyM => KeyToken::Letter('M'),
        IcedCode::KeyN => KeyToken::Letter('N'),
        IcedCode::KeyO => KeyToken::Letter('O'),
        IcedCode::KeyP => KeyToken::Letter('P'),
        IcedCode::KeyQ => KeyToken::Letter('Q'),
        IcedCode::KeyR => KeyToken::Letter('R'),
        IcedCode::KeyS => KeyToken::Letter('S'),
        IcedCode::KeyT => KeyToken::Letter('T'),
        IcedCode::KeyU => KeyToken::Letter('U'),
        IcedCode::KeyV => KeyToken::Letter('V'),
        IcedCode::KeyW => KeyToken::Letter('W'),
        IcedCode::KeyX => KeyToken::Letter('X'),
        IcedCode::KeyY => KeyToken::Letter('Y'),
        IcedCode::KeyZ => KeyToken::Letter('Z'),

        IcedCode::Digit0 => KeyToken::Digit(0),
        IcedCode::Digit1 => KeyToken::Digit(1),
        IcedCode::Digit2 => KeyToken::Digit(2),
        IcedCode::Digit3 => KeyToken::Digit(3),
        IcedCode::Digit4 => KeyToken::Digit(4),
        IcedCode::Digit5 => KeyToken::Digit(5),
        IcedCode::Digit6 => KeyToken::Digit(6),
        IcedCode::Digit7 => KeyToken::Digit(7),
        IcedCode::Digit8 => KeyToken::Digit(8),
        IcedCode::Digit9 => KeyToken::Digit(9),

        IcedCode::Numpad0 => KeyToken::NumpadDigit(0),
        IcedCode::Numpad1 => KeyToken::NumpadDigit(1),
        IcedCode::Numpad2 => KeyToken::NumpadDigit(2),
        IcedCode::Numpad3 => KeyToken::NumpadDigit(3),
        IcedCode::Numpad4 => KeyToken::NumpadDigit(4),
        IcedCode::Numpad5 => KeyToken::NumpadDigit(5),
        IcedCode::Numpad6 => KeyToken::NumpadDigit(6),
        IcedCode::Numpad7 => KeyToken::NumpadDigit(7),
        IcedCode::Numpad8 => KeyToken::NumpadDigit(8),
        IcedCode::Numpad9 => KeyToken::NumpadDigit(9),
        IcedCode::NumpadAdd => KeyToken::NumpadAdd,
        IcedCode::NumpadSubtract => KeyToken::NumpadSubtract,
        IcedCode::NumpadMultiply => KeyToken::NumpadMultiply,
        IcedCode::NumpadDivide => KeyToken::NumpadDivide,
        IcedCode::NumpadDecimal => KeyToken::NumpadDecimal,
        IcedCode::NumpadEnter => KeyToken::NumpadEnter,

        IcedCode::Space => KeyToken::Space,
        IcedCode::Tab => KeyToken::Tab,
        IcedCode::Enter => KeyToken::Enter,
        IcedCode::Backspace => KeyToken::Backspace,
        IcedCode::Delete => KeyToken::Delete,
        IcedCode::Insert => KeyToken::Insert,
        IcedCode::Home => KeyToken::Home,
        IcedCode::End => KeyToken::End,
        IcedCode::PageUp => KeyToken::PageUp,
        IcedCode::PageDown => KeyToken::PageDown,
        IcedCode::ArrowUp => KeyToken::Up,
        IcedCode::ArrowDown => KeyToken::Down,
        IcedCode::ArrowLeft => KeyToken::Left,
        IcedCode::ArrowRight => KeyToken::Right,

        IcedCode::Backquote => KeyToken::Backquote,
        IcedCode::Minus => KeyToken::Minus,
        IcedCode::Equal => KeyToken::Equal,
        IcedCode::BracketLeft => KeyToken::BracketLeft,
        IcedCode::BracketRight => KeyToken::BracketRight,
        IcedCode::Backslash => KeyToken::Backslash,
        IcedCode::Semicolon => KeyToken::Semicolon,
        IcedCode::Quote => KeyToken::Quote,
        IcedCode::Comma => KeyToken::Comma,
        IcedCode::Period => KeyToken::Period,
        IcedCode::Slash => KeyToken::Slash,

        _ => return None,
    })
}

fn map_logical_fallback(key: &Key) -> Option<KeyToken> {
    match key.as_ref() {
        Key::Named(named) => Some(match named {
            Named::Control | Named::Shift | Named::Alt | Named::Super => return None,
            Named::F1 => KeyToken::F(1),
            Named::F2 => KeyToken::F(2),
            Named::F3 => KeyToken::F(3),
            Named::F4 => KeyToken::F(4),
            Named::F5 => KeyToken::F(5),
            Named::F6 => KeyToken::F(6),
            Named::F7 => KeyToken::F(7),
            Named::F8 => KeyToken::F(8),
            Named::F9 => KeyToken::F(9),
            Named::F10 => KeyToken::F(10),
            Named::F11 => KeyToken::F(11),
            Named::F12 => KeyToken::F(12),
            Named::Space => KeyToken::Space,
            Named::Tab => KeyToken::Tab,
            Named::Enter => KeyToken::Enter,
            Named::Backspace => KeyToken::Backspace,
            Named::Delete => KeyToken::Delete,
            Named::Insert => KeyToken::Insert,
            Named::Home => KeyToken::Home,
            Named::End => KeyToken::End,
            Named::PageUp => KeyToken::PageUp,
            Named::PageDown => KeyToken::PageDown,
            Named::ArrowUp => KeyToken::Up,
            Named::ArrowDown => KeyToken::Down,
            Named::ArrowLeft => KeyToken::Left,
            Named::ArrowRight => KeyToken::Right,
            _ => return None,
        }),
        Key::Character(ch) => parse_key_token(ch.trim()).ok(),
        Key::Unidentified => None,
    }
}
