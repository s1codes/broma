//! Tests for `src/audio/service.rs` (Background Audio Coordinator Service).

use std::time::Duration;

use iced::futures::StreamExt;
use iced::futures::executor::block_on;

use crate::audio::DEVICE_DISABLED;
use crate::audio::service::AudioServiceGuard;
use crate::daemon_bus::{self, DaemonEvent};
use crate::storage::SlotConfig;
use crate::tests::common::{AUDIO_SERVER_TEST_LOCK, build_test_wav};

#[test]
fn background_audio_service_spawns_handles_commands_and_joins_cleanly() {
    let _guard = AUDIO_SERVER_TEST_LOCK.lock().unwrap();
    let (bus_tx, bus_rx) = daemon_bus::create();

    let mut boot = AudioServiceGuard::spawn(
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        None,
        0,
        100,
        100,
        false, // Closed to tray: must NOT push UI events onto DaemonBus
        bus_tx,
    );

    let handle = boot.guard.handle();
    assert_eq!(handle.active_slots_mask(), 0);
    assert!(!handle.is_previewing());

    let dir = std::env::temp_dir().join("broma_test_bg_audio_service");
    let _ = std::fs::create_dir_all(&dir);
    let wav_path = dir.join("clip.wav");
    std::fs::write(&wav_path, build_test_wav(48_000, 1, 2_400)).unwrap();

    // Sync slot table and trigger while window_open == false: zero UI bus chatter.
    let slots = vec![SlotConfig::default()];
    handle.sync_slots(&slots, 1, 0);
    handle.set_global_volume(0);
    handle.stop_all();

    // Now simulate opening the window (`set_window_open(true)`) and triggering preview on
    // a missing path to confirm errors route cleanly over the unified `DaemonBus`.
    handle.set_window_open(true);
    handle.toggle_preview(dir.join("missing_file.wav"), 0, 1.0);
    std::thread::sleep(Duration::from_millis(25));

    boot.guard.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    // Confirm `DaemonEvent::AudioError` arrived on the unified `DaemonBus` receiver.
    let mut raw_rx = bus_rx_take_for_test(&bus_rx);
    let first_event = block_on(async { raw_rx.next().await });
    assert!(matches!(first_event, Some(DaemonEvent::AudioError(_))));
}

#[test]
fn refresh_devices_reports_errors_and_refreshed_lists_when_window_open() {
    let _guard = AUDIO_SERVER_TEST_LOCK.lock().unwrap();
    let (bus_tx, bus_rx) = daemon_bus::create();

    let mut boot = AudioServiceGuard::spawn(
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        0,
        100,
        100,
        true,
        bus_tx,
    );

    let handle = boot.guard.handle();

    // Selecting a nonexistent virtual cable must surface an explicit AudioError.
    handle.set_virtual_output("Nonexistent_Cable_Endpoint_XYZ");
    std::thread::sleep(Duration::from_millis(25));

    // Refreshing must still report the (now recovered-or-failed) device lists.
    handle.refresh_devices();
    std::thread::sleep(Duration::from_millis(25));

    boot.guard.shutdown();

    let mut raw_rx = bus_rx_take_for_test(&bus_rx);
    let mut saw_audio_error = false;
    let mut saw_devices_refreshed = false;
    for _ in 0..32 {
        let Some(evt) = block_on(async { raw_rx.next().await }) else {
            break;
        };
        match evt {
            DaemonEvent::AudioError(msg) => {
                assert!(
                    msg.contains("Nonexistent_Cable_Endpoint_XYZ"),
                    "unexpected error text: {msg}"
                );
                saw_audio_error = true;
            }
            DaemonEvent::DevicesRefreshed { .. } => saw_devices_refreshed = true,
            _ => {}
        }
        if saw_audio_error && saw_devices_refreshed {
            break;
        }
    }

    assert!(
        saw_audio_error,
        "missing virtual cable must surface AudioError"
    );
    assert!(
        saw_devices_refreshed,
        "RefreshDevices must emit DevicesRefreshed"
    );
}

fn bus_rx_take_for_test(
    bus_rx: &daemon_bus::DaemonReceiver,
) -> iced::futures::channel::mpsc::UnboundedReceiver<DaemonEvent> {
    bus_rx.take_receiver_for_test()
}
