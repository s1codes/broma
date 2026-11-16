//! Tests for `src/hotkeys/platform/windows.rs` (Win32 `RegisterHotKey` backend).

use std::collections::HashSet;

use global_hotkey::GlobalHotKeyManager;

use crate::audio::DEVICE_DISABLED;
use crate::audio::service::AudioServiceGuard;
use crate::daemon_bus;
use crate::hotkeys::ParsedKeybind;
use crate::hotkeys::platform::windows::{WindowsHotkeyEngine, to_win32_hotkey};
use crate::storage::SlotConfig;

fn make_test_win32_engine() -> (WindowsHotkeyEngine, AudioServiceGuard) {
    let (bus_tx, _bus_rx) = daemon_bus::create();
    let boot = AudioServiceGuard::spawn(
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        None,
        0,
        100,
        100,
        false,
        bus_tx.clone(),
    );
    let handle = boot.guard.handle();
    (WindowsHotkeyEngine::new(handle, bus_tx), boot.guard)
}

#[test]
fn maps_all_supported_key_categories_to_distinct_win32_hotkeys() {
    let samples = [
        "F1",
        "F12",
        "F24",
        "A",
        "Z",
        "0",
        "9",
        "Numpad0",
        "Numpad9",
        "NumpadAdd",
        "NumpadSubtract",
        "NumpadMultiply",
        "NumpadDivide",
        "NumpadDecimal",
        "NumpadEnter",
        "Ctrl + Shift + A",
        "Alt + PageUp",
        "Super + PageDown",
        "Ctrl + Space",
        "Shift + Enter",
    ];

    let mut ids = HashSet::new();
    for sample in samples {
        let parsed = ParsedKeybind::parse(sample).unwrap_or_else(|e| panic!("{sample}: {e}"));
        let hk = to_win32_hotkey(parsed)
            .unwrap_or_else(|| panic!("'{sample}' must map to a valid Win32 HotKey"));
        assert!(
            ids.insert(hk.id()),
            "Win32 HotKey ID collided for '{sample}'"
        );
    }
}

#[test]
fn registers_visible_slots_and_maps_pressed_ids() {
    let (mut engine, mut guard) = make_test_win32_engine();
    let slots = vec![
        SlotConfig {
            name: "Airhorn".to_string(),
            stored_file: Some("a.wav".to_string()),
            keybind: Some("Ctrl + Alt + Shift + F7".to_string()),
            ..SlotConfig::default()
        },
        SlotConfig {
            name: "Unassigned".to_string(),
            stored_file: None,
            keybind: Some("Ctrl + Alt + Shift + F8".to_string()),
            ..SlotConfig::default()
        },
        SlotConfig {
            name: "Hidden".to_string(),
            stored_file: Some("b.wav".to_string()),
            keybind: Some("Ctrl + Alt + Shift + F9".to_string()),
            ..SlotConfig::default()
        },
    ];

    engine.sync_slots(&slots, 2);
    assert_eq!(engine.registered.len(), 1);
    assert!(engine.last_error().is_none());

    let expected_hk =
        to_win32_hotkey(ParsedKeybind::parse("Ctrl + Alt + Shift + F7").unwrap()).unwrap();
    assert_eq!(engine.slot_for_hotkey_id(expected_hk.id()), Some(0));

    engine.shutdown();
    guard.shutdown();
}

#[test]
fn surfaces_explicit_error_when_win32_shortcut_is_already_claimed() {
    let external_mgr = GlobalHotKeyManager::new().expect("Win32 GlobalHotKeyManager should init");
    let contested =
        to_win32_hotkey(ParsedKeybind::parse("Ctrl + Alt + Shift + F10").unwrap()).unwrap();
    external_mgr
        .register(contested)
        .expect("external app grab should succeed");

    let (mut engine, mut guard) = make_test_win32_engine();
    let slots = vec![
        SlotConfig {
            name: "Conflicting".to_string(),
            stored_file: Some("a.wav".to_string()),
            keybind: Some("Ctrl + Alt + Shift + F10".to_string()),
            ..SlotConfig::default()
        },
        SlotConfig {
            name: "Healthy".to_string(),
            stored_file: Some("b.wav".to_string()),
            keybind: Some("Ctrl + Alt + Shift + F11".to_string()),
            ..SlotConfig::default()
        },
    ];

    engine.sync_slots(&slots, 2);

    assert!(
        engine.slot_errors.contains_key(&0),
        "conflicting Win32 shortcut must record a per-slot error"
    );
    assert!(engine.last_error().is_some());
    assert!(!engine.slot_errors.contains_key(&1));
    assert_eq!(engine.registered.len(), 1);

    let _ = external_mgr.unregister(contested);
    engine.shutdown();
    guard.shutdown();
}

#[test]
fn suspend_releases_win32_os_grab_so_modal_key_capture_is_not_swallowed() {
    let (mut engine, mut guard) = make_test_win32_engine();
    let combo = "Ctrl + Alt + Shift + F6";
    let hk = to_win32_hotkey(ParsedKeybind::parse(combo).unwrap()).unwrap();

    let slots = vec![SlotConfig {
        name: "Pad 1".to_string(),
        stored_file: Some("a.wav".to_string()),
        keybind: Some(combo.to_string()),
        ..SlotConfig::default()
    }];

    engine.sync_slots(&slots, 1);
    assert_eq!(engine.registered.len(), 1);

    engine.set_suspended(true);
    assert_eq!(engine.registered.len(), 0);
    assert_eq!(engine.slot_for_hotkey_id(hk.id()), None);

    let probe_mgr = GlobalHotKeyManager::new().unwrap();
    probe_mgr
        .register(hk)
        .expect("Win32 grab must be free while WindowsHotkeyEngine is suspended");
    probe_mgr.unregister(hk).unwrap();

    engine.set_suspended(false);
    assert_eq!(engine.registered.len(), 1);
    assert_eq!(engine.slot_for_hotkey_id(hk.id()), Some(0));

    engine.shutdown();
    guard.shutdown();
}
