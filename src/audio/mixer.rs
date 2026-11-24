// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Lock-free, zero-allocation real-time audio callback mixer (`AGENTS.md` §4–5).
//!
//! Strict real-time invariants inside [`Mixer::render_f32`], [`Mixer::render_i16`],
//! and [`Mixer::render_u16`]:
//! - Zero heap allocations (`Voice` slots are pre-allocated inline).
//! - Zero heap deallocations (`Arc<[i16]>` buffers from finished voices are
//!   pushed back through a lock-free `rtrb` graveyard queue to drop on the UI thread).
//! - Zero locks, mutexes, I/O, or logging.
//! - Optional lock-free microphone passthrough (`mic_rx`) passes mic audio
//!   untouched at unity gain while idle and sums soundboard voices only when active.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use rtrb::{Consumer, Producer};

use crate::audio::mic_bridge::MicPassthroughConsumer;

/// Maximum simultaneous voices mixed per output stream.
pub const MAX_VOICES: usize = 16;

/// Identifies whether a voice belongs to a soundboard pad slot or modal preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceTag {
    Slot(usize),
    Preview,
}

/// Commands sent from the UI thread to a stream mixer over an `rtrb` ring buffer.
///
/// `Clone` is intentionally not derived: [`MixerCommand::ReplaceMic`] carries a
/// non-cloneable ring-buffer consumer, so call sites build one command per stream.
pub enum MixerCommand {
    Play {
        tag: VoiceTag,
        samples: Arc<[i16]>,
        clip_channels: u8,
        clip_sample_rate: u32,
        /// Base slot/preview gain in `0.0..=1.0`.
        base_gain: f32,
        pitch: f32,
        follows_global_volume: bool,
    },
    StopTag(VoiceTag),
    StopAll,
    SetGlobalVolume(f32),
    SetStreamVolume(f32),
    UpdatePreview {
        gain: f32,
        pitch: f32,
    },
    /// Hot-swaps the microphone passthrough consumer (isolated mic recovery).
    /// Active soundboard voices are never touched.
    ReplaceMic(Option<MicPassthroughConsumer>),
}

impl std::fmt::Debug for MixerCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Play {
                tag,
                clip_channels,
                clip_sample_rate,
                ..
            } => f
                .debug_struct("Play")
                .field("tag", tag)
                .field("clip_channels", clip_channels)
                .field("clip_sample_rate", clip_sample_rate)
                .finish_non_exhaustive(),
            Self::StopTag(tag) => f.debug_tuple("StopTag").field(tag).finish(),
            Self::StopAll => f.write_str("StopAll"),
            Self::SetGlobalVolume(vol) => f.debug_tuple("SetGlobalVolume").field(vol).finish(),
            Self::SetStreamVolume(vol) => f.debug_tuple("SetStreamVolume").field(vol).finish(),
            Self::UpdatePreview { gain, pitch } => f
                .debug_struct("UpdatePreview")
                .field("gain", gain)
                .field("pitch", pitch)
                .finish(),
            Self::ReplaceMic(mic) => f
                .debug_tuple("ReplaceMic")
                .field(&mic.as_ref().map_or("None", |_| "Some(<mic>)"))
                .finish(),
        }
    }
}

/// Lock-free atomic playback state readable from the UI thread without locking.
#[derive(Default)]
pub struct StreamStatus {
    /// Bit `i` is set while `VoiceTag::Slot(i)` (`i < 64`) is actively playing.
    pub active_slots_mask: AtomicU64,
    pub preview_playing: AtomicBool,
    /// Flipped by `cpal`'s stream error callback (e.g. WASAPI `AUDCLNT_E_DEVICE_INVALIDATED`
    /// when a Windows USB headset is unplugged or default endpoint changes).
    pub stream_faulted: AtomicBool,
    /// Encoded `cpal::ErrorKind` recorded lock-free by the stream error callback (`0` = none).
    pub last_error_code: AtomicU8,
}

impl StreamStatus {
    pub fn clear(&self) {
        self.active_slots_mask.store(0, Ordering::Relaxed);
        self.preview_playing.store(false, Ordering::Relaxed);
        self.stream_faulted.store(false, Ordering::Relaxed);
        self.last_error_code.store(0, Ordering::Relaxed);
    }
}

struct Voice {
    tag: VoiceTag,
    samples: Arc<[i16]>,
    clip_channels: u8,
    clip_sample_rate: u32,
    frame_count: usize,
    /// Current fractional frame cursor inside `samples`.
    cursor: f64,
    /// Frames advanced per output stream frame (`pitch * clip_rate / stream_rate`).
    step: f64,
    effective_gain: f32,
    follows_global_volume: bool,
}

struct MicReceiver {
    rx: Consumer<f32>,
    step: f64,
    frac: f64,
    prev_sample: f32,
    curr_sample: f32,
    primed: bool,
    max_queued_samples: usize,
}

impl MicReceiver {
    fn new(consumer: MicPassthroughConsumer, stream_sample_rate: u32) -> Self {
        let mic_rate = consumer.sample_rate.max(1);
        let out_rate = stream_sample_rate.max(1);
        // Cap queued mic latency at ~30 ms so clock drift between hardware mic and
        // virtual cable never accumulates delay over long gaming sessions.
        let max_queued_samples = ((mic_rate as usize) * 3 / 100).max(128);
        Self {
            rx: consumer.rx,
            step: f64::from(mic_rate) / f64::from(out_rate),
            frac: 0.0,
            prev_sample: 0.0,
            curr_sample: 0.0,
            primed: false,
            max_queued_samples,
        }
    }

    #[inline]
    fn trim_drift(&mut self) {
        let queued = self.rx.slots();
        if queued > self.max_queued_samples {
            let keep = self.max_queued_samples / 2;
            let drop_count = queued - keep;
            if let Ok(chunk) = self.rx.read_chunk(drop_count) {
                chunk.commit_all();
            }
        }
    }

    #[inline]
    fn next_mono_sample(&mut self) -> f32 {
        if !self.primed {
            if self.rx.slots() < 2 {
                if self.rx.slots() == 1 && (self.step - 1.0).abs() < f64::EPSILON {
                    return self.rx.pop().unwrap_or(0.0);
                }
                return 0.0;
            }
            self.prev_sample = self.rx.pop().unwrap_or(0.0);
            self.curr_sample = self.rx.pop().unwrap_or(self.prev_sample);
            self.frac = 0.0;
            self.primed = true;
        }

        while self.frac >= 1.0 {
            self.frac -= 1.0;
            self.prev_sample = self.curr_sample;
            match self.rx.pop() {
                Ok(next) => self.curr_sample = next,
                Err(_) => {
                    self.primed = false;
                    let tail = self.prev_sample;
                    self.prev_sample = 0.0;
                    self.curr_sample = 0.0;
                    return tail;
                }
            }
        }

        let sample = if self.frac == 0.0 {
            self.prev_sample
        } else {
            self.prev_sample + (self.frac as f32) * (self.curr_sample - self.prev_sample)
        };
        self.frac += self.step;
        sample
    }
}

pub struct Mixer {
    voices: [Option<Voice>; MAX_VOICES],
    mic_rx: Option<MicReceiver>,
    stream_channels: usize,
    stream_sample_rate: u32,
    global_volume: f32,
    stream_volume: f32,
    cmd_rx: Consumer<MixerCommand>,
    retired_tx: Producer<Arc<[i16]>>,
    /// Retires replaced mic ring buffers to the UI thread so the audio callback
    /// never deallocates a `Consumer<f32>` ring allocation (`AGENTS.md` §5).
    retired_mic_tx: Option<Producer<Consumer<f32>>>,
    /// Holds one mic consumer when `retired_mic_tx` is momentarily full.
    parked_retired_mic: Option<Consumer<f32>>,
    status: Arc<StreamStatus>,
}

impl Mixer {
    pub fn new(
        stream_channels: u16,
        stream_sample_rate: u32,
        initial_global_volume: f32,
        cmd_rx: Consumer<MixerCommand>,
        retired_tx: Producer<Arc<[i16]>>,
        status: Arc<StreamStatus>,
        mic_passthrough: Option<MicPassthroughConsumer>,
    ) -> Self {
        let safe_rate = stream_sample_rate.max(1);
        Self {
            voices: std::array::from_fn(|_| None),
            mic_rx: mic_passthrough.map(|c| MicReceiver::new(c, safe_rate)),
            stream_channels: usize::from(stream_channels.max(1)),
            stream_sample_rate: safe_rate,
            global_volume: initial_global_volume.clamp(0.0, 1.0),
            stream_volume: 1.0,
            cmd_rx,
            retired_tx,
            retired_mic_tx: None,
            parked_retired_mic: None,
            status,
        }
    }

    /// Wires the off-thread retirement queue for replaced mic consumers.
    pub fn with_retired_mic(mut self, retired_mic_tx: Producer<Consumer<f32>>) -> Self {
        self.retired_mic_tx = Some(retired_mic_tx);
        self
    }

    pub fn render_f32(&mut self, out: &mut [f32]) {
        self.drain_commands();
        out.fill(0.0);

        let voices_active = self.has_active_voices();
        if !voices_active && self.mic_rx.is_none() {
            self.publish_status();
            return;
        }

        if let Some(mic) = &mut self.mic_rx {
            mic.trim_drift();
        }

        let ch = self.stream_channels;
        if !voices_active {
            // Pure untouched unity mic passthrough when no soundboard clip is playing.
            if let Some(mic) = &mut self.mic_rx {
                for frame in out.chunks_exact_mut(ch) {
                    let s = mic.next_mono_sample();
                    frame[0] = s;
                    if ch >= 2 {
                        frame[1] = s;
                    }
                }
            }
            self.publish_status();
            return;
        }

        for frame in out.chunks_exact_mut(ch) {
            let mic_s = self
                .mic_rx
                .as_mut()
                .map_or(0.0, MicReceiver::next_mono_sample);
            let (left, right) = self.mix_next_stereo_frame();
            if ch == 1 {
                frame[0] = (mic_s + (left + right) * 0.5).clamp(-1.0, 1.0);
            } else {
                frame[0] = (mic_s + left).clamp(-1.0, 1.0);
                frame[1] = (mic_s + right).clamp(-1.0, 1.0);
            }
        }

        self.publish_status();
    }

    pub fn render_i16(&mut self, out: &mut [i16]) {
        self.drain_commands();
        out.fill(0);

        let voices_active = self.has_active_voices();
        if !voices_active && self.mic_rx.is_none() {
            self.publish_status();
            return;
        }

        if let Some(mic) = &mut self.mic_rx {
            mic.trim_drift();
        }

        let ch = self.stream_channels;
        for frame in out.chunks_exact_mut(ch) {
            let mic_s = self
                .mic_rx
                .as_mut()
                .map_or(0.0, MicReceiver::next_mono_sample);
            let (left, right) = if voices_active {
                self.mix_next_stereo_frame()
            } else {
                (0.0, 0.0)
            };
            if ch == 1 {
                let mono = (mic_s + (left + right) * 0.5).clamp(-1.0, 1.0);
                frame[0] = (mono * 32767.0) as i16;
            } else {
                frame[0] = ((mic_s + left).clamp(-1.0, 1.0) * 32767.0) as i16;
                frame[1] = ((mic_s + right).clamp(-1.0, 1.0) * 32767.0) as i16;
            }
        }

        self.publish_status();
    }

    pub fn render_u16(&mut self, out: &mut [u16]) {
        self.drain_commands();
        out.fill(32768);

        let voices_active = self.has_active_voices();
        if !voices_active && self.mic_rx.is_none() {
            self.publish_status();
            return;
        }

        if let Some(mic) = &mut self.mic_rx {
            mic.trim_drift();
        }

        let ch = self.stream_channels;
        for frame in out.chunks_exact_mut(ch) {
            let mic_s = self
                .mic_rx
                .as_mut()
                .map_or(0.0, MicReceiver::next_mono_sample);
            let (left, right) = if voices_active {
                self.mix_next_stereo_frame()
            } else {
                (0.0, 0.0)
            };
            if ch == 1 {
                let mono = (mic_s + (left + right) * 0.5).clamp(-1.0, 1.0);
                frame[0] = ((mono * 32767.0) + 32768.0) as u16;
            } else {
                frame[0] = (((mic_s + left).clamp(-1.0, 1.0) * 32767.0) + 32768.0) as u16;
                frame[1] = (((mic_s + right).clamp(-1.0, 1.0) * 32767.0) + 32768.0) as u16;
            }
        }

        self.publish_status();
    }

    fn drain_commands(&mut self) {
        self.flush_parked_retired_mic();

        while let Ok(cmd) = self.cmd_rx.pop() {
            match cmd {
                MixerCommand::Play {
                    tag,
                    samples,
                    clip_channels,
                    clip_sample_rate,
                    base_gain,
                    pitch,
                    follows_global_volume,
                } => {
                    self.start_voice(
                        tag,
                        samples,
                        clip_channels,
                        clip_sample_rate,
                        base_gain,
                        pitch,
                        follows_global_volume,
                    );
                }
                MixerCommand::StopTag(target) => {
                    for slot in &mut self.voices {
                        if slot.as_ref().is_some_and(|v| v.tag == target)
                            && let Some(removed) = slot.take()
                        {
                            let _ = self.retired_tx.push(removed.samples);
                        }
                    }
                }
                MixerCommand::StopAll => {
                    for slot in &mut self.voices {
                        if let Some(removed) = slot.take() {
                            let _ = self.retired_tx.push(removed.samples);
                        }
                    }
                }
                MixerCommand::SetGlobalVolume(vol) => {
                    self.global_volume = vol.clamp(0.0, 1.0);
                    for voice in self.voices.iter_mut().flatten() {
                        if voice.follows_global_volume {
                            voice.effective_gain = self.global_volume;
                        }
                    }
                }
                MixerCommand::SetStreamVolume(vol) => {
                    self.stream_volume = vol.clamp(0.0, 1.0);
                }
                MixerCommand::UpdatePreview { gain, pitch } => {
                    let safe_gain = gain.clamp(0.0, 1.0);
                    let safe_pitch = pitch.clamp(0.25, 4.0);
                    let stream_rate = f64::from(self.stream_sample_rate);
                    for voice in self.voices.iter_mut().flatten() {
                        if voice.tag == VoiceTag::Preview {
                            voice.effective_gain = safe_gain;
                            voice.step = f64::from(safe_pitch)
                                * (f64::from(voice.clip_sample_rate) / stream_rate);
                        }
                    }
                }
                MixerCommand::ReplaceMic(next_mic) => {
                    if let Some(old) = self.mic_rx.take() {
                        self.retire_mic(old.rx);
                    }
                    self.mic_rx = next_mic
                        .map(|consumer| MicReceiver::new(consumer, self.stream_sample_rate));
                }
            }
        }
    }

    #[inline]
    fn retire_mic(&mut self, rx: Consumer<f32>) {
        let Some(tx) = &mut self.retired_mic_tx else {
            // No retirement queue configured (unit tests only): drop here.
            return;
        };
        if let Err(rtrb::PushError::Full(rx)) = tx.push(rx) {
            self.parked_retired_mic = Some(rx);
        }
    }

    #[inline]
    fn flush_parked_retired_mic(&mut self) {
        let Some(parked) = self.parked_retired_mic.take() else {
            return;
        };
        let Some(tx) = &mut self.retired_mic_tx else {
            return;
        };
        if let Err(rtrb::PushError::Full(parked)) = tx.push(parked) {
            self.parked_retired_mic = Some(parked);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_voice(
        &mut self,
        tag: VoiceTag,
        samples: Arc<[i16]>,
        clip_channels: u8,
        clip_sample_rate: u32,
        base_gain: f32,
        pitch: f32,
        follows_global_volume: bool,
    ) {
        let ch = usize::from(clip_channels.max(1));
        let frame_count = samples.len() / ch;
        if frame_count == 0 || clip_sample_rate == 0 {
            let _ = self.retired_tx.push(samples);
            return;
        }

        let mut target_idx: Option<usize> = None;
        let mut free_idx: Option<usize> = None;

        for (idx, slot) in self.voices.iter().enumerate() {
            match slot {
                Some(v) if v.tag == tag => {
                    target_idx = Some(idx);
                    break;
                }
                None if free_idx.is_none() => {
                    free_idx = Some(idx);
                }
                _ => {}
            }
        }

        let idx = target_idx.or(free_idx).unwrap_or(0);
        if let Some(prev) = self.voices[idx].take() {
            let _ = self.retired_tx.push(prev.samples);
        }

        let safe_pitch = pitch.clamp(0.25, 4.0);
        let step = f64::from(safe_pitch)
            * (f64::from(clip_sample_rate) / f64::from(self.stream_sample_rate));
        let clamped_base = base_gain.clamp(0.0, 1.0);
        let effective_gain = if follows_global_volume {
            self.global_volume
        } else {
            clamped_base
        };

        self.voices[idx] = Some(Voice {
            tag,
            samples,
            clip_channels: clip_channels.max(1),
            clip_sample_rate,
            frame_count,
            cursor: 0.0,
            step,
            effective_gain,
            follows_global_volume,
        });
    }

    #[inline]
    fn has_active_voices(&self) -> bool {
        self.voices.iter().any(Option::is_some)
    }

    #[inline]
    fn mix_next_stereo_frame(&mut self) -> (f32, f32) {
        const INV_I16: f32 = 1.0 / 32768.0;
        let mut sum_l = 0.0f32;
        let mut sum_r = 0.0f32;

        for slot in &mut self.voices {
            let Some(v) = slot.as_mut() else {
                continue;
            };

            let idx0 = v.cursor as usize;
            if idx0 >= v.frame_count {
                if let Some(done) = slot.take() {
                    let _ = self.retired_tx.push(done.samples);
                }
                continue;
            }

            let frac = (v.cursor - (idx0 as f64)) as f32;
            let idx1 = (idx0 + 1).min(v.frame_count - 1);
            let gain = v.effective_gain * INV_I16;

            if v.clip_channels == 1 {
                let s0 = f32::from(v.samples[idx0]);
                let s1 = f32::from(v.samples[idx1]);
                let mono = (s0 + frac * (s1 - s0)) * gain;
                sum_l += mono;
                sum_r += mono;
            } else {
                let base0 = idx0 * 2;
                let base1 = idx1 * 2;
                let l0 = f32::from(v.samples[base0]);
                let r0 = f32::from(v.samples[base0 + 1]);
                let l1 = f32::from(v.samples[base1]);
                let r1 = f32::from(v.samples[base1 + 1]);

                sum_l += (l0 + frac * (l1 - l0)) * gain;
                sum_r += (r0 + frac * (r1 - r0)) * gain;
            }

            v.cursor += v.step;
            if (v.cursor as usize) >= v.frame_count
                && let Some(done) = slot.take()
            {
                let _ = self.retired_tx.push(done.samples);
            }
        }

        (sum_l * self.stream_volume, sum_r * self.stream_volume)
    }

    #[inline]
    fn publish_status(&self) {
        if !self.cmd_rx.is_empty() {
            return;
        }

        let mut mask = 0u64;
        let mut preview = false;

        for v in self.voices.iter().flatten() {
            match v.tag {
                VoiceTag::Slot(idx) if idx < 64 => {
                    mask |= 1u64 << idx;
                }
                VoiceTag::Slot(_) => {}
                VoiceTag::Preview => {
                    preview = true;
                }
            }
        }

        self.status.active_slots_mask.store(mask, Ordering::Relaxed);
        self.status
            .preview_playing
            .store(preview, Ordering::Relaxed);
    }
}
