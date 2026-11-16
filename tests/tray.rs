//! Tests for `src/tray/mod.rs` and `src/daemon_bus.rs`.

use crate::audio::DEVICE_DISABLED;
use crate::audio::service::AudioServiceGuard;
use crate::daemon_bus;
use crate::tests::common::AUDIO_SERVER_TEST_LOCK;
use crate::tray::{TrayController, default_icon_rgba};

#[test]
fn placeholder_icon_is_valid_32x32_solid_blue_rgba() {
    let (rgba, width, height) = default_icon_rgba();
    assert_eq!(width, 32);
    assert_eq!(height, 32);
    assert_eq!(rgba.len(), 32 * 32 * 4);
    let (pixels, remainder) = rgba.as_chunks::<4>();
    assert!(remainder.is_empty());
    for pixel in pixels {
        assert_eq!(pixel, &[0x3B, 0x82, 0xF6, 0xFF]);
    }
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
