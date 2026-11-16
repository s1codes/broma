//! Tests for `src/audio/decode.rs`.

use std::io::Write;

use crate::audio::decode::{decode_audio_clip, probe_audio_file};
use crate::tests::common::build_test_wav;

#[test]
fn probes_valid_wav_and_extracts_metadata() {
    let dir = std::env::temp_dir().join("broma_test_probe_valid");
    let _ = std::fs::create_dir_all(&dir);
    let wav_path = dir.join("tone.wav");

    let bytes = build_test_wav(44_100, 2, 22_050); // 500ms stereo
    std::fs::File::create(&wav_path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();

    let info = probe_audio_file(&wav_path).expect("valid wav should probe");
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 2);
    assert_eq!(info.duration_ms, Some(500));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rejects_broken_non_audio_files() {
    let dir = std::env::temp_dir().join("broma_test_probe_invalid");
    let _ = std::fs::create_dir_all(&dir);
    let bad_path = dir.join("broken.mp3");

    std::fs::write(&bad_path, b"not actually an audio bitstream").unwrap();

    let err = probe_audio_file(&bad_path).expect_err("garbage bytes must fail validation");
    assert!(!err.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn decodes_and_resamples_clip_to_target_rate() {
    let dir = std::env::temp_dir().join("broma_test_decode_resample");
    let _ = std::fs::create_dir_all(&dir);
    let wav_path = dir.join("mono_24k.wav");

    let bytes = build_test_wav(24_000, 1, 12_000); // 500ms mono @ 24kHz
    std::fs::write(&wav_path, &bytes).unwrap();

    let clip = decode_audio_clip(&wav_path, 48_000).expect("decode + resample should succeed");
    assert_eq!(clip.sample_rate, 48_000);
    assert_eq!(clip.channels, 1);
    // 12,000 frames @ 24kHz -> ~24,000 frames @ 48kHz
    assert!((clip.frame_count() as isize - 24_000).abs() <= 64);
    assert_eq!(clip.byte_len(), clip.samples.len() * 2);
    assert!((clip.duration_ms() as i64 - 500).abs() <= 5);
    assert!(clip.samples.iter().any(|&s| s.abs() > 8_000));

    let _ = std::fs::remove_dir_all(&dir);
}
