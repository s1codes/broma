//! Tests for `src/audio/platform/windows.rs`.

use crate::audio::platform::windows::{
    WindowsAudioRouter, filter_windows_microphones, is_windows_loopback_input,
    is_windows_virtual_cable_output, sort_and_dedupe_windows_outputs,
};
use crate::audio::{DEVICE_DISABLED, DEVICE_SYSTEM_DEFAULT_MIC};

#[test]
fn recognizes_windows_virtual_cable_outputs_and_defaults() {
    let router = WindowsAudioRouter::new();
    let outputs = vec![
        "Speakers (Realtek High Definition Audio)".to_string(),
        "CABLE Input (VB-Audio Virtual Cable)".to_string(),
        DEVICE_DISABLED.to_string(),
    ];
    assert!(is_windows_virtual_cable_output(
        "CABLE Input (VB-Audio Virtual Cable)"
    ));
    assert!(is_windows_virtual_cable_output(
        "VoiceMeeter Aux Input (VB-Audio VoiceMeeter AUX VAIO)"
    ));
    assert!(!is_windows_virtual_cable_output(
        "Speakers (Realtek High Definition Audio)"
    ));
    assert_eq!(
        router.default_virtual_output(&outputs),
        "CABLE Input (VB-Audio Virtual Cable)"
    );
}

#[test]
fn sorts_virtual_cables_above_speakers_and_deduplicates_wasapi_endpoints() {
    let sorted = sort_and_dedupe_windows_outputs(vec![
        "LG ULTRAGEAR (NVIDIA High Definition Audio)".to_string(),
        "Speakers (Realtek USB Audio)".to_string(),
        "CABLE Input (VB-Audio Virtual Cable)".to_string(),
        "Speakers (Realtek USB Audio)".to_string(),
        "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)".to_string(),
        "".to_string(),
    ]);

    assert_eq!(
        sorted,
        vec![
            "CABLE Input (VB-Audio Virtual Cable)",
            "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)",
            "LG ULTRAGEAR (NVIDIA High Definition Audio)",
            "Speakers (Realtek USB Audio)",
            DEVICE_DISABLED,
        ]
    );
}

#[test]
fn filters_windows_loopback_inputs_to_prevent_feedback() {
    assert!(is_windows_loopback_input(
        "CABLE Output (VB-Audio Virtual Cable)"
    ));
    assert!(is_windows_loopback_input("Stereo Mix (Realtek Audio)"));
    assert!(is_windows_loopback_input("VoiceMeeter Out B1"));
    assert!(!is_windows_loopback_input("Microphone (HyperX QuadCast)"));

    let mics = filter_windows_microphones(vec![
        "CABLE Output (VB-Audio Virtual Cable)".to_string(),
        "Microphone (HyperX QuadCast)".to_string(),
        "Stereo Mix (Realtek Audio)".to_string(),
        "Microphone (HyperX QuadCast)".to_string(),
        "What U Hear (Sound Blaster)".to_string(),
    ]);

    assert_eq!(
        mics,
        vec![
            DEVICE_SYSTEM_DEFAULT_MIC,
            "Microphone (HyperX QuadCast)",
            DEVICE_DISABLED,
        ]
    );
}

#[test]
fn missing_windows_output_returns_explicit_error_not_silent_noop() {
    let host = cpal::default_host();
    let mut router = WindowsAudioRouter::new();
    let res = router.prepare_route(
        &host,
        DEVICE_DISABLED,
        "Nonexistent_WASAPI_Cable_Endpoint_12345",
    );
    match res {
        Ok(_) => panic!("missing Windows output endpoint must return Err, not succeed silently"),
        Err(msg) => assert!(msg.contains("Nonexistent_WASAPI_Cable_Endpoint_12345")),
    }
}

#[cfg(target_os = "windows")]
#[test]
fn wasapi_reports_explicit_warning_when_selected_mic_is_missing() {
    let host = cpal::default_host();
    let mut router = WindowsAudioRouter::new();
    let outputs = router.list_virtual_outputs(&host);

    // If WASAPI exposes at least one real output endpoint on this Windows/Wine host,
    // verify that a missing microphone keeps the output alive AND reports a warning.
    if let Some(first_real_out) = outputs.iter().find(|o| o.as_str() != DEVICE_DISABLED) {
        let route = router
            .prepare_route(&host, "Unplugged_USB_Mic_9999", first_real_out)
            .expect("valid output sink should still open even when mic is missing")
            .expect("route should be Some");
        assert!(route.mic_bridge.is_none());
        let warning = route
            .route_warning
            .expect("missing Windows microphone must produce an explicit warning");
        assert!(warning.contains("Unplugged_USB_Mic_9999"));
    }
}
