//! Tests for `src/audio/mod.rs` and audio submodules.

pub mod cache;
pub mod decode;
pub mod mixer;
pub mod platform;
pub mod service;

use std::sync::atomic::Ordering;

use crate::audio::{
    AudioEngine, DEVICE_DISABLED, DEVICE_SYSTEM_DEFAULT_MIC, DEVICE_SYSTEM_DEFAULT_OUTPUT,
};
use crate::tests::common::{AUDIO_SERVER_TEST_LOCK, build_test_wav};

#[test]
fn bootstrap_populates_device_lists_and_clean_idle_cache() {
    let _guard = AUDIO_SERVER_TEST_LOCK.lock().unwrap();
    let boot = AudioEngine::bootstrap(
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        None,
        0,
        100,
        100,
    );
    assert_eq!(
        boot.microphones.first().map(String::as_str),
        Some(DEVICE_SYSTEM_DEFAULT_MIC)
    );
    assert_eq!(
        boot.microphones.last().map(String::as_str),
        Some(DEVICE_DISABLED)
    );
    assert_eq!(
        boot.virtual_outputs.last().map(String::as_str),
        Some(DEVICE_DISABLED)
    );
    assert_eq!(
        boot.monitor_outputs.first().map(String::as_str),
        Some(DEVICE_SYSTEM_DEFAULT_OUTPUT)
    );
    assert_eq!(
        boot.monitor_outputs.last().map(String::as_str),
        Some(DEVICE_DISABLED)
    );
    assert_eq!(boot.engine.cache.used_bytes, 0);
    assert_eq!(boot.engine.active_slots_mask(), 0);
    assert!(!boot.engine.is_previewing());
}

#[test]
fn end_to_end_play_and_stop_on_default_or_headless() {
    let _guard = AUDIO_SERVER_TEST_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join("broma_test_e2e_audio");
    let _ = std::fs::create_dir_all(&dir);
    let wav_path = dir.join("blip.wav");
    let wav_bytes = build_test_wav(48_000, 2, 2_400);
    std::fs::write(&wav_path, &wav_bytes).unwrap();

    let mut boot = AudioEngine::bootstrap(
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        None,
        0,
        100,
        100,
    );
    if boot.engine.monitor.is_some() {
        let started = boot
            .engine
            .toggle_slot(0, &wav_path, Some(0), 0, 1.0)
            .expect("playing valid clip on active monitor should succeed");
        assert!(started.is_some());
        assert!(boot.engine.is_slot_playing(0));
        assert!(boot.engine.cache.used_bytes > 0);

        let stopped = boot
            .engine
            .toggle_slot(0, &wav_path, Some(0), 0, 1.0)
            .expect("stopping active slot should succeed");
        assert!(stopped.is_none());
        assert!(!boot.engine.is_slot_playing(0));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recovers_automatically_when_wasapi_or_host_stream_faults() {
    let _guard = AUDIO_SERVER_TEST_LOCK.lock().unwrap();
    let mut boot = AudioEngine::bootstrap(
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        None,
        0,
        100,
        100,
    );

    if let Some(monitor) = &boot.engine.monitor {
        // Simulate WASAPI `AUDCLNT_E_DEVICE_INVALIDATED` firing the lock-free error callback.
        monitor.status.stream_faulted.store(true, Ordering::Relaxed);
        assert!(
            boot.engine.recover_if_faulted(),
            "faulted stream must trigger automatic recovery"
        );
        if let Some(reopened) = &boot.engine.monitor {
            assert!(!reopened.status.stream_faulted.load(Ordering::Relaxed));
        }
    } else {
        assert!(!boot.engine.recover_if_faulted());
    }
}
