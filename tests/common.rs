//! Shared deterministic test utilities used across audio and storage tests.

use std::f32::consts::TAU;
use std::sync::Mutex;

/// Serializes tests that talk to the live system PipeWire / PulseAudio daemon
/// so parallel test threads do not sweep each other's virtual mic mid-assertion.
pub static AUDIO_SERVER_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Builds a valid 16-bit PCM WAV file in memory for deterministic tests.
pub fn build_test_wav(sample_rate: u32, channels: u16, frames: usize) -> Vec<u8> {
    let block_align = channels * 2;
    let byte_rate = sample_rate * u32::from(block_align);
    let data_bytes = (frames * usize::from(block_align)) as u32;
    let riff_size = 36 + data_bytes;

    let mut buf = Vec::with_capacity(44 + data_bytes as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format = 1
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes()); // 16-bit
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());

    for frame_idx in 0..frames {
        let phase = (frame_idx as f32 / sample_rate as f32) * 440.0 * TAU;
        let sample = (phase.sin() * 16000.0) as i16;
        for _ in 0..channels {
            buf.extend_from_slice(&sample.to_le_bytes());
        }
    }
    buf
}
