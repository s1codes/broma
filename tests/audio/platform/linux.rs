//! Tests for `src/audio/platform/linux.rs`.

use crate::audio::DEVICE_SYSTEM_DEFAULT_MIC;
use crate::audio::platform::linux::{
    BROMA_MIC_DESC, BROMA_MIC_NAME, DEVICE_BROMA_VIRTUAL_MIC, LinuxAudioRouter, PulseControlConn,
    is_internal_or_monitor_name, is_stale_broma_module_arg,
};
use crate::tests::common::AUDIO_SERVER_TEST_LOCK;

#[test]
fn detects_stale_broma_module_arguments_accurately() {
    assert!(is_stale_broma_module_arg(
        "sink_name=broma_soundboard_sink rate=48000"
    ));
    assert!(is_stale_broma_module_arg(
        "source_name=broma_virtual_mic master=broma_soundboard_sink.monitor"
    ));
    assert!(!is_stale_broma_module_arg("sink_name=other_app_sink"));
}

#[test]
fn filters_monitor_and_broma_feedback_names() {
    assert!(is_internal_or_monitor_name(
        "Monitor of Razer BlackShark V2 X USB"
    ));
    assert!(is_internal_or_monitor_name("Broma Virtual Microphone"));
    assert!(!is_internal_or_monitor_name(
        "Razer BlackShark V2 X USB Analog Stereo"
    ));
}

#[test]
fn provisions_and_cleans_up_virtual_mic_when_server_present() {
    let _guard = AUDIO_SERVER_TEST_LOCK.lock().unwrap();
    if pulseaudio::socket_path_from_env().is_none() {
        return;
    }

    let host = cpal::default_host();
    let mut router = LinuxAudioRouter::new();
    let outputs = router.list_virtual_outputs(&host);
    assert!(outputs.iter().any(|o| o == DEVICE_BROMA_VIRTUAL_MIC));

    // Provision with default mic passthrough (`module-loopback` + `module-remap-source` + `module-null-sink`).
    let route = router
        .prepare_route(&host, DEVICE_SYSTEM_DEFAULT_MIC, DEVICE_BROMA_VIRTUAL_MIC)
        .expect("provisioning Broma Virtual Mic with mic passthrough should succeed");
    assert!(route.is_some());

    // Verify `"Broma Virtual Microphone"` actually exists as a real non-monitor Source!
    let mut conn = PulseControlConn::connect().unwrap();
    let sources = conn.list_sources().unwrap();
    let created = sources
        .iter()
        .find(|s| s.name.to_string_lossy() == BROMA_MIC_NAME)
        .expect("broma_virtual_mic source must exist in server");
    assert!(created.monitor_of_sink_index.is_none());
    assert_eq!(
        created
            .description
            .as_ref()
            .map(|d| d.to_string_lossy().into_owned())
            .as_deref(),
        Some(BROMA_MIC_DESC)
    );
    drop(conn);

    // Teardown must unload all modules cleanly so the virtual mic disappears.
    router.teardown();
    let mut conn_after = PulseControlConn::connect().unwrap();
    let sources_after = conn_after.list_sources().unwrap();
    assert!(
        !sources_after
            .iter()
            .any(|s| s.name.to_string_lossy() == BROMA_MIC_NAME)
    );
}
