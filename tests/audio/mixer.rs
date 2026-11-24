//! Tests for `src/audio/mixer.rs`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rtrb::RingBuffer;

use crate::audio::mic_bridge::MicPassthroughConsumer;
use crate::audio::mixer::{Mixer, MixerCommand, StreamStatus, VoiceTag};

#[test]
fn mixes_voices_tracks_status_and_retires_finished_arcs() {
    let (mut cmd_tx, cmd_rx) = RingBuffer::new(32);
    let (retired_tx, mut retired_rx) = RingBuffer::new(32);
    let status = Arc::new(StreamStatus::default());

    let mut mixer = Mixer::new(
        2,
        48_000,
        0.8,
        cmd_rx,
        retired_tx,
        Arc::clone(&status),
        None,
    );

    let clip: Arc<[i16]> = Arc::from(vec![16_384i16; 4]);
    cmd_tx
        .push(MixerCommand::Play {
            tag: VoiceTag::Slot(2),
            samples: Arc::clone(&clip),
            clip_channels: 1,
            clip_sample_rate: 48_000,
            base_gain: 1.0,
            pitch: 1.0,
            follows_global_volume: false,
        })
        .unwrap();

    let mut out = vec![0.0f32; 4];
    mixer.render_f32(&mut out);
    assert!((out[0] - 0.5).abs() < 0.001);
    assert!((out[1] - 0.5).abs() < 0.001);
    assert_eq!(status.active_slots_mask.load(Ordering::Relaxed), 1u64 << 2);
    assert!(retired_rx.is_empty());

    let mut tail = vec![0.0f32; 8];
    mixer.render_f32(&mut tail);
    assert_eq!(status.active_slots_mask.load(Ordering::Relaxed), 0);
    let retired = retired_rx.pop().expect("finished clip Arc must be retired");
    assert!(Arc::ptr_eq(&clip, &retired));
}

#[test]
fn pitch_multiplier_doubles_step_speed_and_clamps_summed_voices() {
    let (mut cmd_tx, cmd_rx) = RingBuffer::new(32);
    let (retired_tx, _retired_rx) = RingBuffer::new(32);
    let status = Arc::new(StreamStatus::default());

    let mut mixer = Mixer::new(1, 48_000, 1.0, cmd_rx, retired_tx, status, None);

    let loud_clip: Arc<[i16]> = Arc::from(vec![30_000i16; 8]);
    for slot in [0usize, 1usize] {
        cmd_tx
            .push(MixerCommand::Play {
                tag: VoiceTag::Slot(slot),
                samples: Arc::clone(&loud_clip),
                clip_channels: 1,
                clip_sample_rate: 48_000,
                base_gain: 1.0,
                pitch: 2.0,
                follows_global_volume: true,
            })
            .unwrap();
    }

    let mut out = vec![0.0f32; 4];
    mixer.render_f32(&mut out);
    assert!(out.iter().all(|&s| (0.99..=1.0).contains(&s)));

    let mut after = vec![1.0f32; 4];
    mixer.render_f32(&mut after);
    assert!(after.iter().all(|&s| s == 0.0));
}

#[test]
fn mic_passthrough_is_bit_exact_when_idle_and_sums_when_playing() {
    let (mut cmd_tx, cmd_rx) = RingBuffer::new(32);
    let (retired_tx, _retired_rx) = RingBuffer::new(32);
    let status = Arc::new(StreamStatus::default());
    let (mut mic_tx, mic_rx) = RingBuffer::new(64);

    let mut mixer = Mixer::new(
        2,
        48_000,
        1.0,
        cmd_rx,
        retired_tx,
        status,
        Some(MicPassthroughConsumer {
            rx: mic_rx,
            sample_rate: 48_000,
        }),
    );

    let input_samples = [0.125f32, -0.25f32, 0.375f32, -0.5f32];
    for &s in &input_samples {
        mic_tx.push(s).unwrap();
    }

    // Idle: output stereo frames must equal the untouched mic samples bit-for-bit.
    let mut idle_out = vec![0.0f32; 8];
    mixer.render_f32(&mut idle_out);
    for (idx, &expected) in input_samples.iter().enumerate() {
        assert_eq!(idle_out[idx * 2], expected);
        assert_eq!(idle_out[idx * 2 + 1], expected);
    }

    // Active pad: mic (0.25) + clip (16384 / 32768 = 0.5) => 0.75.
    for _ in 0..4 {
        mic_tx.push(0.25f32).unwrap();
    }
    cmd_tx
        .push(MixerCommand::Play {
            tag: VoiceTag::Slot(0),
            samples: Arc::from(vec![16_384i16; 4]),
            clip_channels: 1,
            clip_sample_rate: 48_000,
            base_gain: 1.0,
            pitch: 1.0,
            follows_global_volume: false,
        })
        .unwrap();

    let mut mixed_out = vec![0.0f32; 4];
    mixer.render_f32(&mut mixed_out);
    assert!((mixed_out[0] - 0.75).abs() < 0.001);
    assert!((mixed_out[1] - 0.75).abs() < 0.001);
}

#[test]
fn replace_mic_hot_swaps_passthrough_while_voices_keep_playing() {
    let (mut cmd_tx, cmd_rx) = RingBuffer::new(32);
    let (retired_tx, _retired_rx) = RingBuffer::new(32);
    let (retired_mic_tx, mut retired_mic_rx) = RingBuffer::new(8);
    let status = Arc::new(StreamStatus::default());
    let (old_mic_tx, old_mic_rx) = RingBuffer::new(64);

    let mut mixer = Mixer::new(
        2,
        48_000,
        1.0,
        cmd_rx,
        retired_tx,
        Arc::clone(&status),
        Some(MicPassthroughConsumer {
            rx: old_mic_rx,
            sample_rate: 48_000,
        }),
    )
    .with_retired_mic(retired_mic_tx);

    // Long soundboard voice (64 mono frames) so it cannot finish during the swap.
    cmd_tx
        .push(MixerCommand::Play {
            tag: VoiceTag::Slot(0),
            samples: Arc::from(vec![16_384i16; 64]),
            clip_channels: 1,
            clip_sample_rate: 48_000,
            base_gain: 1.0,
            pitch: 1.0,
            follows_global_volume: false,
        })
        .unwrap();

    let mut first = vec![0.0f32; 4];
    mixer.render_f32(&mut first);
    assert_eq!(status.active_slots_mask.load(Ordering::Relaxed), 1);
    assert!(retired_mic_rx.is_empty());

    // Hot-swap to a fresh mic ring while the voice plays.
    let (mut new_mic_tx, new_mic_rx) = RingBuffer::new(64);
    cmd_tx
        .push(MixerCommand::ReplaceMic(Some(MicPassthroughConsumer {
            rx: new_mic_rx,
            sample_rate: 48_000,
        })))
        .unwrap();
    for _ in 0..8 {
        new_mic_tx.push(0.125f32).unwrap();
    }

    let mut out = vec![0.0f32; 4];
    mixer.render_f32(&mut out);

    // Voice still playing and the new mic ring is the one being mixed.
    assert_eq!(status.active_slots_mask.load(Ordering::Relaxed), 1);
    assert!((out[0] - 0.625).abs() < 0.01, "got {}", out[0]);
    assert!((out[1] - 0.625).abs() < 0.01, "got {}", out[1]);

    // Old ring is retired to the UI thread instead of being dropped in the callback.
    let retired = retired_mic_rx
        .pop()
        .expect("replaced mic ring must be retired");
    drop(retired);
    assert!(old_mic_tx.is_abandoned());
}

#[test]
fn replace_mic_none_stops_passthrough_but_keeps_voices_playing() {
    let (mut cmd_tx, cmd_rx) = RingBuffer::new(32);
    let (retired_tx, _retired_rx) = RingBuffer::new(32);
    let (retired_mic_tx, mut retired_mic_rx) = RingBuffer::new(8);
    let status = Arc::new(StreamStatus::default());
    let (mut mic_tx, mic_rx) = RingBuffer::new(64);

    let mut mixer = Mixer::new(
        2,
        48_000,
        1.0,
        cmd_rx,
        retired_tx,
        Arc::clone(&status),
        Some(MicPassthroughConsumer {
            rx: mic_rx,
            sample_rate: 48_000,
        }),
    )
    .with_retired_mic(retired_mic_tx);

    cmd_tx
        .push(MixerCommand::Play {
            tag: VoiceTag::Slot(1),
            samples: Arc::from(vec![16_384i16; 64]),
            clip_channels: 1,
            clip_sample_rate: 48_000,
            base_gain: 1.0,
            pitch: 1.0,
            follows_global_volume: false,
        })
        .unwrap();
    cmd_tx.push(MixerCommand::ReplaceMic(None)).unwrap();

    for _ in 0..8 {
        mic_tx.push(0.5f32).unwrap();
    }

    let mut out = vec![0.0f32; 4];
    mixer.render_f32(&mut out);

    // Mic passthrough is gone; only the soundboard voice remains.
    assert!((out[0] - 0.5).abs() < 0.001, "got {}", out[0]);
    assert!((out[1] - 0.5).abs() < 0.001, "got {}", out[1]);
    assert_eq!(status.active_slots_mask.load(Ordering::Relaxed), 1u64 << 1);

    let retired = retired_mic_rx
        .pop()
        .expect("disabled mic ring must be retired");
    drop(retired);
    assert!(mic_tx.is_abandoned());
}

#[test]
fn renders_7_1_surround_8ch_wasapi_mix_format_cleanly() {
    let (mut cmd_tx, cmd_rx) = RingBuffer::new(16);
    let (retired_tx, _retired_rx) = RingBuffer::new(16);
    let status = Arc::new(StreamStatus::default());

    // Many Windows gaming headsets (Razer Spatial / SteelSeries Sonar / Logitech)
    // expose an 8-channel 7.1 `GetMixFormat` in WASAPI shared mode.
    let mut mixer = Mixer::new(8, 48_000, 1.0, cmd_rx, retired_tx, status, None);
    cmd_tx
        .push(MixerCommand::Play {
            tag: VoiceTag::Slot(0),
            samples: Arc::from(vec![16_384i16, -16_384i16, 16_384i16, -16_384i16]),
            clip_channels: 2,
            clip_sample_rate: 48_000,
            base_gain: 1.0,
            pitch: 1.0,
            follows_global_volume: false,
        })
        .unwrap();

    let mut out_f32 = vec![99.0f32; 16]; // 2 frames * 8 channels
    mixer.render_f32(&mut out_f32);
    assert!((out_f32[0] - 0.5).abs() < 0.001);
    assert!((out_f32[1] + 0.5).abs() < 0.001);
    // Channels 2..8 (Center, LFE, Rear/Side) must be strictly zeroed, never stale!
    assert!(out_f32[2..8].iter().all(|&s| s == 0.0));

    // Also verify U16 silence midpoint (32768) on surround channels when idle.
    let mut idle_u16 = vec![0u16; 8];
    let (_tx2, rx2) = RingBuffer::new(8);
    let (ret_tx2, _ret_rx2) = RingBuffer::new(8);
    let mut u16_mixer = Mixer::new(
        8,
        48_000,
        1.0,
        rx2,
        ret_tx2,
        Arc::new(StreamStatus::default()),
        None,
    );
    u16_mixer.render_u16(&mut idle_u16);
    assert!(idle_u16.iter().all(|&s| s == 32_768));
}

#[test]
fn resamples_mismatched_mic_rate_and_clamps_long_session_clock_drift() {
    let (_cmd_tx, cmd_rx) = RingBuffer::new(16);
    let (retired_tx, _retired_rx) = RingBuffer::new(16);
    let status = Arc::new(StreamStatus::default());
    let (mut mic_tx, mic_rx) = RingBuffer::new(16_384);

    // 16 kHz USB/headset mic feeding a 48 kHz VB-Cable stream (`max_queued_samples = 480`).
    let mut mixer = Mixer::new(
        2,
        48_000,
        1.0,
        cmd_rx,
        retired_tx,
        status,
        Some(MicPassthroughConsumer {
            rx: mic_rx,
            sample_rate: 16_000,
        }),
    );

    // Flood 8,000 samples (500 ms of accumulated clock drift) into the mic ring.
    for _ in 0..8_000 {
        mic_tx.push(0.4f32).unwrap();
    }

    let mut out = vec![0.0f32; 96]; // 48 stereo frames (1 ms @ 48 kHz)
    mixer.render_f32(&mut out);

    // Output must carry clean interpolated mic signal AND excess drift must have been trimmed
    // from 8,000 samples down to ~240 samples (< 20 ms latency)!
    assert!(out.iter().all(|&s| (s - 0.4).abs() < 0.01));
    let remaining_queued = 16_384 - mic_tx.slots();
    assert!(
        remaining_queued <= 480,
        "clock drift was not trimmed: {remaining_queued} samples still queued"
    );
}

#[test]
fn stream_volume_scales_soundboard_voices_without_attenuating_mic_passthrough() {
    let (mut cmd_tx, cmd_rx) = RingBuffer::new(32);
    let (retired_tx, _retired_rx) = RingBuffer::new(32);
    let status = Arc::new(StreamStatus::default());
    let (mut mic_tx, mic_rx) = RingBuffer::new(64);

    let mut mixer = Mixer::new(
        2,
        48_000,
        1.0,
        cmd_rx,
        retired_tx,
        status,
        Some(MicPassthroughConsumer {
            rx: mic_rx,
            sample_rate: 48_000,
        }),
    );

    for _ in 0..4 {
        mic_tx.push(0.25f32).unwrap();
    }

    cmd_tx.push(MixerCommand::SetStreamVolume(0.5)).unwrap();
    cmd_tx
        .push(MixerCommand::Play {
            tag: VoiceTag::Slot(0),
            samples: Arc::from(vec![16_384i16; 4]), // 0.5 full-scale
            clip_channels: 1,
            clip_sample_rate: 48_000,
            base_gain: 1.0,
            pitch: 1.0,
            follows_global_volume: false,
        })
        .unwrap();

    // Mic stays at unity (0.25), while soundboard (0.5) is scaled by stream_volume (0.5) => 0.25.
    // Combined output = 0.25 + 0.25 = 0.50.
    let mut out = vec![0.0f32; 4];
    mixer.render_f32(&mut out);
    assert!((out[0] - 0.5).abs() < 0.001, "got {}", out[0]);
    assert!((out[1] - 0.5).abs() < 0.001, "got {}", out[1]);
}
