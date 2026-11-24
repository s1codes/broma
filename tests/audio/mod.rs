//! Tests for `src/audio/mod.rs` and audio submodules.

pub mod cache;
pub mod decode;
pub mod mixer;
pub mod platform;
pub mod service;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cpal::{ErrorKind, SampleFormat, SupportedBufferSize, SupportedStreamConfigRange};

use crate::audio::mixer::StreamStatus;
use crate::audio::{
    AudioEngine, DEVICE_DISABLED, DEVICE_SYSTEM_DEFAULT_MIC, DEVICE_SYSTEM_DEFAULT_OUTPUT,
    RecoveryBackoff, decode_error_kind_label, encode_error_kind, is_fatal_stream_error,
    select_best_supported_config,
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

#[test]
fn recovery_backoff_progresses_from_immediate_to_capped_delays() {
    let t0 = Instant::now();
    let mut backoff = RecoveryBackoff::new();

    // First attempt after a fault is immediate.
    assert!(backoff.can_attempt(t0));
    backoff.record_attempt(t0);
    assert!(!backoff.can_attempt(t0 + Duration::from_millis(249)));
    assert!(backoff.can_attempt(t0 + Duration::from_millis(250)));

    let t1 = t0 + Duration::from_millis(250);
    backoff.record_attempt(t1);
    assert!(!backoff.can_attempt(t1 + Duration::from_millis(499)));
    assert!(backoff.can_attempt(t1 + Duration::from_millis(500)));

    let t2 = t1 + Duration::from_millis(500);
    backoff.record_attempt(t2);
    assert!(!backoff.can_attempt(t2 + Duration::from_millis(999)));
    assert!(backoff.can_attempt(t2 + Duration::from_millis(1_000)));

    let t3 = t2 + Duration::from_millis(1_000);
    backoff.record_attempt(t3);
    assert!(!backoff.can_attempt(t3 + Duration::from_millis(1_999)));
    assert!(backoff.can_attempt(t3 + Duration::from_millis(2_000)));

    let t4 = t3 + Duration::from_millis(2_000);
    backoff.record_attempt(t4);
    assert!(
        !backoff.can_attempt(t4 + Duration::from_millis(1_999)),
        "backoff must cap at 2 s"
    );
    assert!(backoff.can_attempt(t4 + Duration::from_millis(2_000)));

    backoff.reset();
    assert!(backoff.can_attempt(t4), "reset restores immediate recovery");
}

#[test]
fn recovery_backoff_reports_whether_recovery_has_been_attempted() {
    let t0 = Instant::now();
    let mut backoff = RecoveryBackoff::new();
    assert!(
        !backoff.has_attempted(),
        "a route that never opened must not be retried at boot"
    );

    // The CI regression: an absent device at boot must not look "recoverable".
    assert!(
        !backoff.needs_reopen(false, true),
        "a never-opened endpoint must stay inert until an explicit refresh"
    );
    assert!(
        backoff.needs_reopen(true, false),
        "a faulted stream always needs a reopen"
    );

    backoff.record_attempt(t0);
    assert!(
        backoff.has_attempted(),
        "a failed reopen must keep the route retry-armed"
    );
    assert!(
        backoff.needs_reopen(false, true),
        "an endpoint lost after a failed reopen must be retried"
    );

    backoff.reset();
    assert!(
        !backoff.has_attempted(),
        "reset clears retry-armed state for a fresh device selection"
    );
    assert!(!backoff.needs_reopen(false, true));
}

#[test]
fn recovery_backoff_forgets_failures_after_settle_window() {
    let t0 = Instant::now();
    let mut backoff = RecoveryBackoff::new();
    backoff.record_attempt(t0);
    backoff.record_attempt(t0);
    assert!(!backoff.can_attempt(t0 + Duration::from_millis(250)));

    let settled = t0 + Duration::from_secs(5);
    assert!(backoff.can_attempt(settled));
    backoff.record_attempt(settled);

    // Treated as a first failure again, so the next wait is 250 ms, not 1000+.
    assert!(!backoff.can_attempt(settled + Duration::from_millis(249)));
    assert!(backoff.can_attempt(settled + Duration::from_millis(250)));
}

#[test]
fn faulted_monitor_recovery_reports_error_once_and_respects_backoff() {
    let _guard = AUDIO_SERVER_TEST_LOCK.lock().unwrap();
    let mut boot = AudioEngine::bootstrap(
        Some(DEVICE_DISABLED),
        Some(DEVICE_DISABLED),
        None,
        0,
        100,
        100,
    );

    let Some(monitor) = &boot.engine.monitor else {
        return;
    };

    monitor.status.last_error_code.store(
        encode_error_kind(ErrorKind::StreamInvalidated),
        Ordering::Relaxed,
    );
    monitor.status.stream_faulted.store(true, Ordering::Relaxed);

    assert!(boot.engine.recover_if_faulted());
    let reported = boot
        .engine
        .take_pending_device_error()
        .expect("faulted stream must surface a diagnostic");
    assert!(reported.contains("Monitor"), "got: {reported}");
    assert!(reported.contains("StreamInvalidated"), "got: {reported}");
    assert!(boot.engine.take_pending_device_error().is_none());

    // Re-faulting immediately must be suppressed by the recovery backoff.
    if let Some(reopened) = &boot.engine.monitor {
        reopened
            .status
            .stream_faulted
            .store(true, Ordering::Relaxed);
        assert!(
            !boot.engine.recover_if_faulted(),
            "backoff must block a rapid repeat re-open"
        );
    }
}

#[test]
fn non_fatal_stream_errors_do_not_fault_while_fatal_ones_do() {
    assert!(!is_fatal_stream_error(ErrorKind::Xrun));
    assert!(!is_fatal_stream_error(ErrorKind::DeviceChanged));
    assert!(!is_fatal_stream_error(ErrorKind::RealtimeDenied));

    let fatal_kinds = [
        ErrorKind::DeviceNotAvailable,
        ErrorKind::StreamInvalidated,
        ErrorKind::HostUnavailable,
        ErrorKind::DeviceBusy,
        ErrorKind::PermissionDenied,
        ErrorKind::ResourceExhausted,
        ErrorKind::UnsupportedConfig,
        ErrorKind::UnsupportedOperation,
        ErrorKind::InvalidInput,
        ErrorKind::BackendError,
        ErrorKind::Other,
    ];
    for kind in fatal_kinds {
        assert!(is_fatal_stream_error(kind), "expected {kind:?} to be fatal");
    }
}

#[test]
fn encode_and_decode_error_kind_round_trips_distinctly_for_fatal_kinds() {
    let fatal_kinds = [
        ErrorKind::DeviceNotAvailable,
        ErrorKind::StreamInvalidated,
        ErrorKind::HostUnavailable,
        ErrorKind::DeviceBusy,
        ErrorKind::PermissionDenied,
        ErrorKind::ResourceExhausted,
        ErrorKind::UnsupportedConfig,
        ErrorKind::UnsupportedOperation,
        ErrorKind::InvalidInput,
        ErrorKind::BackendError,
        ErrorKind::Other,
    ];

    let mut codes = std::collections::HashSet::new();
    let mut labels = std::collections::HashSet::new();
    for kind in fatal_kinds {
        let code = encode_error_kind(kind);
        assert_ne!(code, 0, "fatal kind {kind:?} must encode to non-zero");
        assert!(codes.insert(code), "duplicate code {code} for {kind:?}");
        let label = decode_error_kind_label(code);
        assert!(!label.is_empty());
        assert_ne!(label, "None");
        assert_ne!(label, "Unknown");
        assert!(labels.insert(label), "duplicate label {label}");
    }

    assert!(!decode_error_kind_label(0).is_empty());
    assert!(!decode_error_kind_label(255).is_empty());
}

#[test]
fn stream_status_last_error_code_defaults_to_zero_and_clears() {
    let status = StreamStatus::default();
    assert_eq!(status.last_error_code.load(Ordering::Relaxed), 0);

    status.last_error_code.store(2, Ordering::Relaxed);
    status.stream_faulted.store(true, Ordering::Relaxed);
    status.clear();

    assert_eq!(status.last_error_code.load(Ordering::Relaxed), 0);
    assert!(!status.stream_faulted.load(Ordering::Relaxed));
}

#[test]
fn select_best_supported_config_avoids_8khz_trap_and_ranks_rates_channels_formats() {
    let buf = SupportedBufferSize::Unknown;
    let wasapi_ranges = vec![
        // Classic WASAPI trap: 8 kHz mono/stereo I16 appears first in the list.
        SupportedStreamConfigRange::new(2, 8_000, 8_000, buf, SampleFormat::I16),
        SupportedStreamConfigRange::new(1, 48_000, 48_000, buf, SampleFormat::F32),
        SupportedStreamConfigRange::new(2, 44_100, 44_100, buf, SampleFormat::F32),
        SupportedStreamConfigRange::new(2, 48_000, 48_000, buf, SampleFormat::I16),
        SupportedStreamConfigRange::new(2, 48_000, 48_000, buf, SampleFormat::F32),
    ];

    let best_out = select_best_supported_config(wasapi_ranges.clone(), 2)
        .expect("should pick a valid output config");
    assert_eq!(best_out.sample_rate(), 48_000);
    assert_eq!(best_out.channels(), 2);
    assert_eq!(best_out.sample_format(), SampleFormat::F32);

    let best_in =
        select_best_supported_config(wasapi_ranges, 1).expect("should pick a valid input config");
    assert_eq!(best_in.sample_rate(), 48_000);
    assert_eq!(best_in.channels(), 1);
    assert_eq!(best_in.sample_format(), SampleFormat::F32);

    // Channel preference must outrank format preference: mono I16 wins for a
    // mono mic request even though the stereo candidate is F32.
    let channel_ranges = vec![
        SupportedStreamConfigRange::new(2, 48_000, 48_000, buf, SampleFormat::F32),
        SupportedStreamConfigRange::new(1, 48_000, 48_000, buf, SampleFormat::I16),
    ];
    let mono = select_best_supported_config(channel_ranges.clone(), 1).unwrap();
    assert_eq!(mono.channels(), 1);
    assert_eq!(mono.sample_format(), SampleFormat::I16);
    let stereo = select_best_supported_config(channel_ranges, 2).unwrap();
    assert_eq!(stereo.channels(), 2);
    assert_eq!(stereo.sample_format(), SampleFormat::F32);

    // When 48 kHz is absent, 44.1 kHz must beat 8 kHz even when 8 kHz comes first.
    let no_48k_ranges = vec![
        SupportedStreamConfigRange::new(2, 8_000, 8_000, buf, SampleFormat::F32),
        SupportedStreamConfigRange::new(2, 44_100, 44_100, buf, SampleFormat::I16),
    ];
    let fallback =
        select_best_supported_config(no_48k_ranges, 2).expect("should pick 44.1 kHz fallback");
    assert_eq!(fallback.sample_rate(), 44_100);
    assert_eq!(fallback.sample_format(), SampleFormat::I16);

    // When neither 48 kHz nor 44.1 kHz is present, >= 44.1 kHz closer to 48 kHz beats >44.1 far
    // and beats < 44.1 kHz.
    let high_rate_ranges = vec![
        SupportedStreamConfigRange::new(2, 32_000, 32_000, buf, SampleFormat::F32),
        SupportedStreamConfigRange::new(2, 96_000, 96_000, buf, SampleFormat::F32),
        SupportedStreamConfigRange::new(2, 88_200, 88_200, buf, SampleFormat::I16),
    ];
    let best_high = select_best_supported_config(high_rate_ranges, 2).unwrap();
    assert_eq!(best_high.sample_rate(), 88_200);

    // Below 44.1 kHz, higher rate wins over 8 kHz.
    let low_rate_ranges = vec![
        SupportedStreamConfigRange::new(2, 8_000, 8_000, buf, SampleFormat::F32),
        SupportedStreamConfigRange::new(2, 32_000, 32_000, buf, SampleFormat::I16),
    ];
    let best_low = select_best_supported_config(low_rate_ranges, 2).unwrap();
    assert_eq!(best_low.sample_rate(), 32_000);
}
