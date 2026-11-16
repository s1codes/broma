// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Bounded LRU cache for decoded 16-bit audio clips (`AGENTS.md` §5, Milestone 7).
//!
//! Clips are decoded lazily on first playback rather than at startup so a
//! 50-clip library uses ~0 bytes of PCM RAM while idle. Once decoded, clips are
//! retained up to `max_bytes` (16 MB by default) with least-recently-used eviction.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::audio::decode::{DecodedClip, decode_audio_clip};

/// Default memory cap for cached decoded clips: 16 MB (~90s of stereo 48 kHz 16-bit PCM).
/// Keeps total process RSS comfortably under the 30 MB budget (`AGENTS.md` §2).
pub const DEFAULT_CACHE_CAP_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    path: PathBuf,
    sample_rate: u32,
}

struct CacheEntry {
    key: CacheKey,
    clip: DecodedClip,
}

pub struct LruClipCache {
    /// Front = least-recently used, Back = most-recently used.
    /// Small linear scan over <= ~30–50 entries avoids extra hash/node heap overhead.
    entries: VecDeque<CacheEntry>,
    pub(crate) used_bytes: usize,
    max_bytes: usize,
}

impl Default for LruClipCache {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_CAP_BYTES)
    }
}

impl LruClipCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            used_bytes: 0,
            max_bytes,
        }
    }

    /// Returns a cached clip on hit (promoting it to MRU), or decodes from disk
    /// on miss and inserts into the bounded LRU cache.
    pub fn get_or_decode(
        &mut self,
        path: &Path,
        target_sample_rate: u32,
    ) -> Result<DecodedClip, String> {
        if let Some(hit) = self.get(path, target_sample_rate) {
            return Ok(hit);
        }

        let decoded = decode_audio_clip(path, target_sample_rate)?;
        self.insert(path.to_path_buf(), target_sample_rate, decoded.clone());
        Ok(decoded)
    }

    pub fn get(&mut self, path: &Path, sample_rate: u32) -> Option<DecodedClip> {
        let pos = self
            .entries
            .iter()
            .position(|e| e.key.sample_rate == sample_rate && e.key.path == path)?;

        let entry = self.entries.remove(pos)?;
        let clip = entry.clip.clone();
        self.entries.push_back(entry);
        Some(clip)
    }

    pub fn insert(&mut self, path: PathBuf, sample_rate: u32, clip: DecodedClip) {
        let clip_bytes = clip.byte_len();

        // Remove any existing entry for the same key first.
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.key.sample_rate == sample_rate && e.key.path == path)
            && let Some(removed) = self.entries.remove(pos)
        {
            self.used_bytes = self.used_bytes.saturating_sub(removed.clip.byte_len());
        }

        // Single clips larger than the entire cache cap are played directly without
        // evicting the whole cache or exceeding `max_bytes`.
        if clip_bytes > self.max_bytes {
            return;
        }

        while self.used_bytes + clip_bytes > self.max_bytes {
            let Some(oldest) = self.entries.pop_front() else {
                break;
            };
            self.used_bytes = self.used_bytes.saturating_sub(oldest.clip.byte_len());
        }

        self.used_bytes += clip_bytes;
        self.entries.push_back(CacheEntry {
            key: CacheKey { path, sample_rate },
            clip,
        });
    }

    pub fn invalidate_path(&mut self, path: &Path) {
        let mut idx = 0;
        while idx < self.entries.len() {
            if self.entries[idx].key.path == path {
                if let Some(removed) = self.entries.remove(idx) {
                    self.used_bytes = self.used_bytes.saturating_sub(removed.clip.byte_len());
                }
            } else {
                idx += 1;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.used_bytes = 0;
    }
}
