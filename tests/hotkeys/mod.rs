//! Tests for `src/hotkeys/mod.rs` and hotkey platform submodules.

pub mod platform;

use iced::keyboard::key::{Code as IcedCode, Physical};
use iced::keyboard::{Key, Modifiers as IcedModifiers};

use crate::hotkeys::{KeyToken, ParsedKeybind, capture_iced_keybind, find_conflicting_slot};
use crate::storage::SlotConfig;

#[test]
fn captures_physical_numpad_and_shifted_digits_without_symbol_corruption() {
    // Shift + Digit1 must capture `"Shift + 1"`, NOT `"Shift + !"`.
    let shifted = capture_iced_keybind(
        &Physical::Code(IcedCode::Digit1),
        &Key::Character("!".into()),
        IcedModifiers::SHIFT,
    )
    .unwrap();
    assert_eq!(shifted, "Shift + 1");

    // NumpadAdd must capture `"NumpadAdd"`, NOT `"+"` which would break '+' splitting.
    let num_plus = capture_iced_keybind(
        &Physical::Code(IcedCode::NumpadAdd),
        &Key::Character("+".into()),
        IcedModifiers::CTRL,
    )
    .unwrap();
    assert_eq!(num_plus, "Ctrl + NumpadAdd");
    let reparsed = ParsedKeybind::parse(&num_plus).unwrap();
    assert!(reparsed.ctrl);
    assert_eq!(reparsed.key, KeyToken::NumpadAdd);
}

#[test]
fn parses_legacy_pgup_alias_and_detects_duplicate_slot_bindings() {
    let slots = vec![
        SlotConfig {
            keybind: Some("Ctrl + PgUp".to_string()),
            ..SlotConfig::default()
        },
        SlotConfig::default(),
    ];

    assert_eq!(find_conflicting_slot(&slots, 1, "Ctrl + PageUp"), Some(0));
    assert_eq!(find_conflicting_slot(&slots, 0, "Ctrl + PageUp"), None);
}
