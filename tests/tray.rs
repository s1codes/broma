//! Tests for `src/tray/mod.rs` and `src/daemon_bus.rs`.

use std::collections::HashSet;

use crate::audio::DEVICE_DISABLED;
use crate::audio::service::AudioServiceGuard;
use crate::daemon_bus;
use crate::tests::common::AUDIO_SERVER_TEST_LOCK;
use crate::tray::{TrayController, decode_png_rgba, default_icon_rgba};

#[test]
fn bundled_app_icon_decodes_valid_64x64_rgba_for_tray_and_window() {
    let (rgba, width, height) = default_icon_rgba().expect("bundled 64x64.png should decode");
    assert_eq!(width, 64);
    assert_eq!(height, 64);
    assert_eq!(rgba.len(), 64 * 64 * 4);

    let (pixels, remainder) = rgba.as_chunks::<4>();
    assert!(remainder.is_empty());
    assert_eq!(pixels[0], [0x00, 0x00, 0x12, 0xFF]);

    let distinct_colors: HashSet<[u8; 4]> = pixels.iter().copied().collect();
    assert!(
        distinct_colors.len() > 1,
        "bundled icon should contain artwork with multiple colors, not a solid block"
    );

    tray_icon::Icon::from_rgba(rgba.clone(), width, height)
        .expect("tray_icon::Icon::from_rgba should accept decoded bundled icon");
    iced::window::icon::from_rgba(rgba, width, height)
        .expect("iced::window::icon::from_rgba should accept decoded bundled icon");
}

#[test]
fn decode_png_rgba_rejects_corrupt_bytes() {
    assert!(decode_png_rgba(b"invalid png").is_err());
}

#[test]
fn tray_controller_respects_disabled_preference_and_shuts_down_cleanly() {
    let _guard = AUDIO_SERVER_TEST_LOCK.lock().unwrap();
    let (bus_tx, _bus_rx) = daemon_bus::create();
    let mut boot = AudioServiceGuard::spawn(
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        None,
        0,
        100,
        100,
        true,
        bus_tx.clone(),
    );

    let mut tray = TrayController::new(false, boot.guard.handle(), bus_tx);
    assert!(
        !tray.should_close_to_tray(),
        "when close_to_tray is disabled, window close must exit instead of hiding"
    );

    // Toggling off again and shutting down must be idempotent and never panic.
    tray.set_enabled(false);
    tray.shutdown();
    boot.guard.shutdown();
}
