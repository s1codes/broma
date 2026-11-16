//! Tests for `src/hotkeys/platform/linux.rs`.

use crate::hotkeys::ParsedKeybind;
use crate::hotkeys::platform::linux::{parse_slot_shortcut_id, to_xdg_shortcut_trigger};

#[test]
fn formats_xdg_triggers_with_exact_xkbcommon_keysyms() {
    let combo = ParsedKeybind::parse("Ctrl + Shift + A").unwrap();
    assert_eq!(to_xdg_shortcut_trigger(combo), "CTRL+SHIFT+a");

    let numpad = ParsedKeybind::parse("Alt + Numpad1").unwrap();
    assert_eq!(to_xdg_shortcut_trigger(numpad), "ALT+KP_1");

    let nav = ParsedKeybind::parse("Super + PageUp").unwrap();
    assert_eq!(to_xdg_shortcut_trigger(nav), "LOGO+Page_Up");

    let fn_key = ParsedKeybind::parse("F9").unwrap();
    assert_eq!(to_xdg_shortcut_trigger(fn_key), "F9");
}

#[test]
fn parses_slot_shortcut_ids_cleanly() {
    assert_eq!(parse_slot_shortcut_id("slot_0"), Some(0));
    assert_eq!(parse_slot_shortcut_id("slot_47"), Some(47));
    assert_eq!(parse_slot_shortcut_id("invalid"), None);
}
