// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Lock-free hardware microphone capture bridge (`cpal` input -> `rtrb::RingBuffer<f32>`).
//!
//! Used whenever Broma performs in-process microphone passthrough into a virtual
//! cable output (always on Windows VB-Cable/VoiceMeeter, and as a fallback on
//! non-managed external sinks).
//!
//! Input callback rules (`AGENTS.md` §5):
//! - Downmixes multi-channel mic input to mono `f32` inline.
//! - Pushes into a pre-allocated `rtrb::Producer<f32>` without locks, allocations, or I/O.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{
    BufferSize, Device, SampleFormat, Stream, StreamConfig, SupportedBufferSize,
    SupportedStreamConfig,
};
use rtrb::{Consumer, Producer, RingBuffer};

/// Target low-latency capture callback size (~5.3 ms at 48 kHz).
const PREFERRED_INPUT_BUFFER_FRAMES: u32 = 256;

/// Pre-allocated ring capacity (~340 ms at 48 kHz); mixer trims drift above ~30 ms.
const MIC_RING_CAPACITY: usize = 16_384;

/// Consumer half handed to the Virtual Cable output [`crate::audio::mixer::Mixer`].
pub struct MicPassthroughConsumer {
    pub rx: Consumer<f32>,
    pub sample_rate: u32,
}

/// Active `cpal` microphone capture stream feeding a lock-free ring buffer.
pub struct MicBridge {
    _stream: Stream,
    faulted: Arc<AtomicBool>,
}

impl MicBridge {
    pub fn is_faulted(&self) -> bool {
        self.faulted.load(Ordering::Relaxed)
    }

    /// Opens a low-latency capture stream on `mic_device` and returns both the
    /// live stream handle and the lock-free consumer for the output mixer.
    pub fn open(mic_device: Device) -> Result<(Self, MicPassthroughConsumer), String> {
        let supported = select_input_config(&mic_device)?;
        let sample_format = supported.sample_format();
        let sample_rate = supported.sample_rate();
        let channels = supported.channels();

        let mut low_latency_cfg: StreamConfig = supported.into();
        if let SupportedBufferSize::Range { min, max } = supported.buffer_size() {
            let frames = PREFERRED_INPUT_BUFFER_FRAMES.clamp(*min, *max);
            low_latency_cfg.buffer_size = BufferSize::Fixed(frames);
        }

        match build_input_stream(
            &mic_device,
            low_latency_cfg,
            sample_format,
            channels,
            sample_rate,
        ) {
            Ok(pair) => Ok(pair),
            Err(_) if low_latency_cfg.buffer_size != BufferSize::Default => {
                let default_cfg: StreamConfig = supported.into();
                build_input_stream(
                    &mic_device,
                    default_cfg,
                    sample_format,
                    channels,
                    sample_rate,
                )
            }
            Err(err) => Err(err),
        }
    }
}

fn select_input_config(device: &Device) -> Result<SupportedStreamConfig, String> {
    if let Ok(default_cfg) = device.default_input_config()
        && matches!(
            default_cfg.sample_format(),
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
        )
    {
        return Ok(default_cfg);
    }

    let ranges = device
        .supported_input_configs()
        .map_err(|e| format!("Failed to query microphone configs: {e}"))?;

    let mut best = None;
    for range in ranges {
        if !matches!(
            range.sample_format(),
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
        ) {
            continue;
        }
        let rate = 48_000.clamp(range.min_sample_rate(), range.max_sample_rate());
        let candidate = range.with_sample_rate(rate);
        if candidate.channels() == 1 {
            return Ok(candidate);
        }
        if best.is_none() {
            best = Some(candidate);
        }
    }

    best.ok_or_else(|| "No supported PCM capture format on microphone".to_string())
}

fn build_input_stream(
    device: &Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    channels: u16,
    sample_rate: u32,
) -> Result<(MicBridge, MicPassthroughConsumer), String> {
    let (mut tx, rx) = RingBuffer::<f32>::new(MIC_RING_CAPACITY);
    let ch = usize::from(channels.max(1));
    let inv_ch = 1.0 / (ch as f32);
    let faulted = Arc::new(AtomicBool::new(false));
    let err_flag = Arc::clone(&faulted);

    // Lock-free fault flag records WASAPI/device invalidation without allocating (`AGENTS.md` §5).
    let err_fn = move |_err| {
        err_flag.store(true, Ordering::Relaxed);
    };

    let stream = match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| push_f32_frames(data, ch, inv_ch, &mut tx),
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to open F32 mic stream: {e}"))?,
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| push_i16_frames(data, ch, inv_ch, &mut tx),
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to open I16 mic stream: {e}"))?,
        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| push_u16_frames(data, ch, inv_ch, &mut tx),
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to open U16 mic stream: {e}"))?,
        other => return Err(format!("Unsupported microphone sample format: {other:?}")),
    };

    stream
        .play()
        .map_err(|e| format!("Failed to start microphone capture: {e}"))?;

    Ok((
        MicBridge {
            _stream: stream,
            faulted,
        },
        MicPassthroughConsumer { rx, sample_rate },
    ))
}

#[inline]
fn push_f32_frames(data: &[f32], ch: usize, inv_ch: f32, tx: &mut Producer<f32>) {
    if ch == 1 {
        for &s in data {
            if tx.push(s).is_err() {
                break;
            }
        }
    } else {
        for frame in data.chunks_exact(ch) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += s;
            }
            if tx.push(sum * inv_ch).is_err() {
                break;
            }
        }
    }
}

#[inline]
fn push_i16_frames(data: &[i16], ch: usize, inv_ch: f32, tx: &mut Producer<f32>) {
    const INV_I16: f32 = 1.0 / 32768.0;
    if ch == 1 {
        for &s in data {
            if tx.push(f32::from(s) * INV_I16).is_err() {
                break;
            }
        }
    } else {
        let scale = INV_I16 * inv_ch;
        for frame in data.chunks_exact(ch) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += f32::from(s);
            }
            if tx.push(sum * scale).is_err() {
                break;
            }
        }
    }
}

#[inline]
fn push_u16_frames(data: &[u16], ch: usize, inv_ch: f32, tx: &mut Producer<f32>) {
    const INV_I16: f32 = 1.0 / 32768.0;
    if ch == 1 {
        for &s in data {
            let centered = (f32::from(s) - 32768.0) * INV_I16;
            if tx.push(centered).is_err() {
                break;
            }
        }
    } else {
        let scale = INV_I16 * inv_ch;
        for frame in data.chunks_exact(ch) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += f32::from(s) - 32768.0;
            }
            if tx.push(sum * scale).is_err() {
                break;
            }
        }
    }
}
