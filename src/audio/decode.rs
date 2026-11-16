// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Audio probing, decoding (`symphonia`), and sample-rate conversion (`rubato`).
//!
//! All work in this module runs off the real-time audio callback thread.
//! Decoded audio is stored as compact 16-bit PCM (`Arc<[i16]>`) in 1ch (mono)
//! or 2ch (stereo) layout (`AGENTS.md` §4–5).

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::audio::{Audio, GenericAudioBufferRef};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

/// Safety cap for a single decoded clip in memory (64 MB of `i16` PCM, ~5.5 min stereo @ 48 kHz).
pub const MAX_DECODED_CLIP_BYTES: usize = 64 * 1024 * 1024;

/// Metadata extracted when validating an audio file on import (`AGENTS.md` §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioProbeInfo {
    pub duration_ms: Option<u64>,
    pub sample_rate: u32,
    pub channels: u16,
    pub format_name: String,
}

/// Shared, immutable 16-bit PCM buffer ready for lock-free playback across streams.
#[derive(Debug, Clone)]
pub struct DecodedClip {
    pub samples: Arc<[i16]>,
    pub sample_rate: u32,
    /// `1` (mono) or `2` (stereo).
    pub channels: u8,
}

impl DecodedClip {
    pub fn frame_count(&self) -> usize {
        let ch = usize::from(self.channels.max(1));
        self.samples.len() / ch
    }

    pub fn byte_len(&self) -> usize {
        self.samples.len() * std::mem::size_of::<i16>()
    }

    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        ((self.frame_count() as u64) * 1000) / u64::from(self.sample_rate)
    }
}

/// Probes and validates an audio file using `symphonia`.
///
/// Rejects unreadable, unsupported, or zero-frame/corrupted files with a clear
/// user-facing error message (`AGENTS.md` §8 step 2).
pub fn probe_audio_file(path: &Path) -> Result<AudioProbeInfo, String> {
    let file = File::open(path).map_err(|e| format!("Could not open audio file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|_| "Unsupported or corrupted audio file format".to_string())?;

    let format_short = format.format_info().short_name.to_ascii_uppercase();

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "File does not contain a playable audio track".to_string())?;

    let track_id = track.id;
    let time_base = track.time_base;
    let track_duration = track.duration;
    let track_num_frames = track.num_frames;

    let Some(CodecParameters::Audio(audio_params)) = track.codec_params.clone() else {
        return Err("Audio track is missing codec parameters".to_string());
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .map_err(|_| "Unsupported audio codec".to_string())?;

    let mut sample_rate = audio_params.sample_rate.unwrap_or(0);
    let mut channels = audio_params
        .channels
        .as_ref()
        .map(|c| c.count() as u16)
        .unwrap_or(0);

    // Decode at least one non-empty audio packet to confirm the stream is genuinely playable.
    let mut decoded_first_packet = false;
    while let Some(packet) = format
        .next_packet()
        .map_err(|e| format!("Corrupted audio stream: {e}"))?
    {
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf_ref) => {
                if buf_ref.frames() > 0 && buf_ref.num_planes() > 0 {
                    sample_rate = buf_ref.spec().rate();
                    channels = buf_ref.num_planes() as u16;
                    decoded_first_packet = true;
                    break;
                }
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
            }
            Err(e) => return Err(format!("Failed to decode audio stream: {e}")),
        }
    }

    if !decoded_first_packet || sample_rate == 0 || channels == 0 {
        return Err("Audio file contains no playable audio samples".to_string());
    }

    let duration_ms = compute_duration_ms(time_base, track_duration, track_num_frames, sample_rate);

    let format_name = if format_short.is_empty() {
        path.extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_uppercase())
            .unwrap_or_else(|| "AUDIO".to_string())
    } else {
        format_short
    };

    Ok(AudioProbeInfo {
        duration_ms,
        sample_rate,
        channels,
        format_name,
    })
}

/// Decodes an audio file into a shared `Arc<[i16]>` buffer, resampling to
/// `target_sample_rate` with `rubato` when needed (`AGENTS.md` §3–5).
pub fn decode_audio_clip(path: &Path, target_sample_rate: u32) -> Result<DecodedClip, String> {
    let file = File::open(path).map_err(|e| format!("Could not open audio file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|_| "Unsupported or corrupted audio file format".to_string())?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "File does not contain a playable audio track".to_string())?;

    let track_id = track.id;
    let track_num_frames = track.num_frames;
    let Some(CodecParameters::Audio(audio_params)) = track.codec_params.clone() else {
        return Err("Audio track is missing codec parameters".to_string());
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .map_err(|_| "Unsupported audio codec".to_string())?;

    let mut src_sample_rate = audio_params.sample_rate.unwrap_or(0);
    let mut out_channels: usize = 0;

    let max_i16_samples = MAX_DECODED_CLIP_BYTES / std::mem::size_of::<i16>();
    let initial_frames = track_num_frames
        .map(|n| (n as usize).min(max_i16_samples / 2))
        .unwrap_or(0);
    let mut pcm_f32: Vec<f32> = Vec::with_capacity(initial_frames.saturating_mul(2));
    let mut packet_scratch: Vec<f32> = Vec::new();

    while let Some(packet) = format
        .next_packet()
        .map_err(|e| format!("Corrupted audio stream: {e}"))?
    {
        if packet.track_id != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(buf_ref) => {
                let frames = buf_ref.frames();
                let planes = buf_ref.num_planes();
                if frames == 0 || planes == 0 {
                    continue;
                }

                if out_channels == 0 {
                    src_sample_rate = buf_ref.spec().rate();
                    out_channels = planes.min(2);
                }

                // Guard against multi-hour files exceeding the in-memory safety budget.
                let projected_samples = pcm_f32.len() + frames * out_channels;
                if projected_samples > max_i16_samples {
                    return Err(
                        "Audio clip exceeds the maximum supported clip length (~5.5 minutes)"
                            .to_string(),
                    );
                }

                append_buffer_f32(buf_ref, &mut packet_scratch, &mut pcm_f32, out_channels);
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
            }
            Err(e) => return Err(format!("Failed to decode audio stream: {e}")),
        }
    }

    if pcm_f32.is_empty() || src_sample_rate == 0 || out_channels == 0 {
        return Err("Audio file contains no playable audio samples".to_string());
    }

    let final_rate = if target_sample_rate > 0 && target_sample_rate != src_sample_rate {
        pcm_f32 =
            resample_interleaved_f32(&pcm_f32, out_channels, src_sample_rate, target_sample_rate)?;
        target_sample_rate
    } else {
        src_sample_rate
    };

    if pcm_f32.len() > max_i16_samples {
        return Err("Resampled audio clip exceeds the memory cap".to_string());
    }

    let mut pcm_i16 = Vec::with_capacity(pcm_f32.len());
    for &sample in &pcm_f32 {
        let clamped = sample.clamp(-1.0, 1.0);
        let scaled = (clamped * 32767.0).round() as i16;
        pcm_i16.push(scaled);
    }

    Ok(DecodedClip {
        samples: Arc::from(pcm_i16),
        sample_rate: final_rate,
        channels: out_channels as u8,
    })
}

fn append_buffer_f32(
    buf_ref: GenericAudioBufferRef<'_>,
    scratch: &mut Vec<f32>,
    dst: &mut Vec<f32>,
    out_channels: usize,
) {
    let planes = buf_ref.num_planes();
    let frames = buf_ref.frames();

    match buf_ref {
        GenericAudioBufferRef::U8(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::U16(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::U24(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::U32(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::S8(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::S16(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::S24(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::S32(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::F32(b) => b.copy_to_vec_interleaved(scratch),
        GenericAudioBufferRef::F64(b) => b.copy_to_vec_interleaved(scratch),
    }

    if planes == out_channels {
        dst.extend_from_slice(scratch);
    } else if out_channels == 1 {
        // Average multi-channel packet down to mono.
        let inv = 1.0 / (planes as f32);
        dst.reserve(frames);
        for frame in scratch.chunks_exact(planes) {
            let sum: f32 = frame.iter().copied().sum();
            dst.push(sum * inv);
        }
    } else {
        // `out_channels == 2`: map mono -> stereo, or take front L/R from surround.
        dst.reserve(frames * 2);
        if planes == 1 {
            for &s in scratch.iter() {
                dst.push(s);
                dst.push(s);
            }
        } else {
            for frame in scratch.chunks_exact(planes) {
                dst.push(frame[0]);
                dst.push(frame[1]);
            }
        }
    }
}

/// Resamples an interleaved `f32` buffer from `src_rate` to `dst_rate` using `rubato::Fft`.
pub fn resample_interleaved_f32(
    interleaved: &[f32],
    channels: usize,
    src_rate: u32,
    dst_rate: u32,
) -> Result<Vec<f32>, String> {
    if src_rate == dst_rate || interleaved.is_empty() {
        return Ok(interleaved.to_vec());
    }
    let frames = interleaved.len() / channels.max(1);
    if frames == 0 {
        return Ok(Vec::new());
    }

    let input_adapter = InterleavedSlice::new(interleaved, channels, frames)
        .map_err(|e| format!("Invalid resample input buffer: {e}"))?;

    let mut resampler = Fft::<f32>::new(
        src_rate as usize,
        dst_rate as usize,
        1024,
        channels,
        FixedSync::Both,
    )
    .map_err(|e| format!("Failed to initialize audio resampler: {e}"))?;

    let resampled = resampler
        .process_all(&input_adapter, frames, None)
        .map_err(|e| format!("Audio resampling failed: {e}"))?;

    Ok(resampled.take_data())
}

fn compute_duration_ms(
    time_base: Option<symphonia::core::units::TimeBase>,
    duration: Option<symphonia::core::units::Duration>,
    num_frames: Option<u64>,
    sample_rate: u32,
) -> Option<u64> {
    if let (Some(tb), Some(dur)) = (time_base, duration)
        && let Some(time) = tb.calc_duration(dur)
        && let Ok(ms) = u64::try_from(time.as_millis())
    {
        return Some(ms);
    }

    if let Some(frames) = num_frames
        && sample_rate > 0
    {
        return Some((frames * 1000) / u64::from(sample_rate));
    }

    None
}
