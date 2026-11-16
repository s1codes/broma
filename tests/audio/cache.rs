//! Tests for `src/audio/cache.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::audio::cache::LruClipCache;
use crate::audio::decode::DecodedClip;

fn dummy_clip(num_i16_samples: usize, sample_rate: u32) -> DecodedClip {
    DecodedClip {
        samples: Arc::from(vec![100i16; num_i16_samples]),
        sample_rate,
        channels: 1,
    }
}

#[test]
fn lru_evicts_least_recently_used_within_byte_cap() {
    // Cap = 300 bytes -> holds at most three 100-byte (50x i16) clips.
    let mut cache = LruClipCache::new(300);

    cache.insert(PathBuf::from("a.wav"), 48_000, dummy_clip(50, 48_000));
    cache.insert(PathBuf::from("b.wav"), 48_000, dummy_clip(50, 48_000));
    cache.insert(PathBuf::from("c.wav"), 48_000, dummy_clip(50, 48_000));
    assert_eq!(cache.used_bytes, 300);

    // Touch `a.wav` so `b.wav` becomes the oldest entry.
    assert!(cache.get(Path::new("a.wav"), 48_000).is_some());

    // Inserting `d.wav` must evict `b.wav`, keeping `a`, `c`, `d`.
    cache.insert(PathBuf::from("d.wav"), 48_000, dummy_clip(50, 48_000));
    assert_eq!(cache.used_bytes, 300);
    assert!(cache.get(Path::new("b.wav"), 48_000).is_none());
    assert!(cache.get(Path::new("a.wav"), 48_000).is_some());
    assert!(cache.get(Path::new("c.wav"), 48_000).is_some());
    assert!(cache.get(Path::new("d.wav"), 48_000).is_some());
}

#[test]
fn oversized_clip_does_not_exceed_cap_or_flush_existing_entries() {
    let mut cache = LruClipCache::new(200);
    cache.insert(PathBuf::from("small.wav"), 48_000, dummy_clip(50, 48_000));
    assert_eq!(cache.used_bytes, 100);

    // 200 samples = 400 bytes > 200-byte cap.
    cache.insert(PathBuf::from("huge.wav"), 48_000, dummy_clip(200, 48_000));
    assert_eq!(cache.used_bytes, 100);
    assert!(cache.get(Path::new("small.wav"), 48_000).is_some());
    assert!(cache.get(Path::new("huge.wav"), 48_000).is_none());
}
