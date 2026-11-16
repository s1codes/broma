// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Persistent configuration, safe grid bounds, and atomic local file storage.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::audio::decode::{AudioProbeInfo, probe_audio_file};

pub const GRID_COLS: usize = 3;
pub const MIN_GRID_ROWS: usize = 1;
pub const DEFAULT_GRID_ROWS: usize = 3; // 3x3 = 9 buttons by default
pub const MAX_GRID_ROWS: usize = 16; // 3x16 = 48 buttons safe ceiling
pub const MAX_SLOTS: usize = GRID_COLS * MAX_GRID_ROWS;

pub const DEFAULT_GLOBAL_VOLUME: u8 = 80;
pub const DEFAULT_STREAM_VOLUME: u8 = 100;
pub const DEFAULT_PITCH_PERCENT: u16 = 100; // 1.00x
pub const MIN_PITCH_PERCENT: u16 = 50; // 0.50x
pub const MAX_PITCH_PERCENT: u16 = 200; // 2.00x

const BACKUP_MAGIC: &[u8; 8] = b"BROMAPK1";

/// Result of importing and validating an external sound file (`AGENTS.md` §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSound {
    pub stored_file: String,
    pub display_file: String,
    pub duration_ms: Option<u64>,
    pub audio_format: String,
}

/// Serializable configuration for a single grid button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotConfig {
    pub name: String,
    /// Relative filename inside `<data_local_dir>/sounds/` once saved, or None.
    pub stored_file: Option<String>,
    /// Original filename shown to the user (display metadata only).
    pub display_file: Option<String>,
    /// Probed clip duration in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Probed container/codec format label (e.g. `"WAV"`, `"MP3"`).
    #[serde(default)]
    pub audio_format: Option<String>,
    /// `None` means this button follows the Global Volume slider.
    pub custom_volume: Option<u8>,
    /// Stored as integer percent (`100` = `1.00x`) so equality checks are exact.
    pub pitch_percent: u16,
    /// Optional shortcut string, e.g. `"F1"` or `"Ctrl + 1"`.
    pub keybind: Option<String>,
}

impl Default for SlotConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            stored_file: None,
            display_file: None,
            duration_ms: None,
            audio_format: None,
            custom_volume: None,
            pitch_percent: DEFAULT_PITCH_PERCENT,
            keybind: None,
        }
    }
}

impl SlotConfig {
    pub fn is_empty(&self) -> bool {
        self.name.trim().is_empty() && self.stored_file.is_none() && self.display_file.is_none()
    }

    pub fn effective_volume(&self, global_volume: u8) -> u8 {
        self.custom_volume.unwrap_or(global_volume)
    }

    pub fn pitch_multiplier(&self) -> f32 {
        f32::from(self.pitch_percent) / 100.0
    }
}

const fn default_close_to_tray() -> bool {
    true
}

const fn default_stream_volume() -> u8 {
    DEFAULT_STREAM_VOLUME
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub grid_rows: usize,
    pub global_volume: u8,
    #[serde(default)]
    pub microphone: Option<String>,
    #[serde(default, alias = "virtual_cable_device")]
    pub output_device: Option<String>,
    #[serde(default)]
    pub monitor_device: Option<String>,
    #[serde(default = "default_stream_volume")]
    pub monitor_volume: u8,
    #[serde(default = "default_stream_volume")]
    pub cable_volume: u8,
    #[serde(default = "default_close_to_tray")]
    pub close_to_tray: bool,
    pub slots: Vec<SlotConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut slots = Vec::with_capacity(GRID_COLS * DEFAULT_GRID_ROWS);
        slots.resize_with(GRID_COLS * DEFAULT_GRID_ROWS, SlotConfig::default);

        Self {
            grid_rows: DEFAULT_GRID_ROWS,
            global_volume: DEFAULT_GLOBAL_VOLUME,
            microphone: None,
            output_device: None,
            monitor_device: None,
            monitor_volume: DEFAULT_STREAM_VOLUME,
            cable_volume: DEFAULT_STREAM_VOLUME,
            close_to_tray: true,
            slots,
        }
    }
}

impl AppConfig {
    pub fn sanitize(&mut self) {
        self.grid_rows = self.grid_rows.clamp(MIN_GRID_ROWS, MAX_GRID_ROWS);
        self.global_volume = self.global_volume.min(100);
        self.monitor_volume = self.monitor_volume.min(100);
        self.cable_volume = self.cable_volume.min(100);

        if self.slots.len() > MAX_SLOTS {
            self.slots.truncate(MAX_SLOTS);
        }
        let needed = self.grid_rows * GRID_COLS;
        if self.slots.len() < needed {
            self.slots.resize_with(needed, SlotConfig::default);
        }

        for slot in &mut self.slots {
            if let Some(v) = &mut slot.custom_volume {
                *v = (*v).min(100);
            }
            slot.pitch_percent = slot
                .pitch_percent
                .clamp(MIN_PITCH_PERCENT, MAX_PITCH_PERCENT);
        }
    }
}

/// Fixed ProjectDirs identifiers — never change once shipped (`AGENTS.md` §7).
pub fn data_dir() -> Option<PathBuf> {
    ProjectDirs::from("com", "broma", "Broma").map(|dirs| dirs.data_local_dir().to_path_buf())
}

pub fn load_config() -> AppConfig {
    let Some(base) = data_dir() else {
        return AppConfig::default();
    };
    let path = base.join("library.json");
    let Ok(bytes) = fs::read(path) else {
        return AppConfig::default();
    };
    let Ok(mut cfg) = serde_json::from_slice::<AppConfig>(&bytes) else {
        return AppConfig::default();
    };
    cfg.sanitize();
    cfg
}

pub fn save_config(cfg: &AppConfig) -> Result<(), String> {
    let base = data_dir().ok_or_else(|| "Could not resolve local data directory".to_string())?;
    save_config_in_dir(&base, cfg)
}

pub fn save_config_in_dir(base: &Path, cfg: &AppConfig) -> Result<(), String> {
    fs::create_dir_all(base).map_err(|e| e.to_string())?;

    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let target = base.join("library.json");
    atomic_write_file(&target, json.as_bytes())
}

/// Step 2 of Import Flow (`AGENTS.md` §8): probe and validate with `symphonia`.
pub fn validate_audio_file(source: &Path) -> Result<AudioProbeInfo, String> {
    probe_audio_file(source)
}

/// Step 3 of Import Flow (`AGENTS.md` §8): content hash via `blake3`.
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Steps 4 & 5 of Import Flow (`AGENTS.md` §8): dedupe by hash and copy atomically
/// into `sounds/<hash>.<ext>`.
pub fn copy_sound_atomic(
    sounds_dir: &Path,
    source: &Path,
    bytes: &[u8],
    hash: &str,
) -> Result<String, String> {
    fs::create_dir_all(sounds_dir).map_err(|e| e.to_string())?;

    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bin".to_string());

    let stored_name = format!("{hash}.{ext}");
    let dest = sounds_dir.join(&stored_name);

    // Dedupe: reuse existing file if an identical hash was already imported.
    if !dest.exists() {
        atomic_write_file(&dest, bytes)?;
    }

    Ok(stored_name)
}

/// Writes `bytes` to a unique sibling `.tmp` file, flushes to disk, and renames
/// into `target` with bounded retry for transient Windows `MoveFileExW` locks
/// (e.g. Windows Defender / Search Indexer briefly scanning the file).
pub(crate) fn atomic_write_file(target: &Path, bytes: &[u8]) -> Result<(), String> {
    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    let parent = target
        .parent()
        .ok_or_else(|| "Target path has no parent directory".to_string())?;
    let base_name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("broma_data");
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".{base_name}.{}.{seq}.tmp", std::process::id()));

    let write_res = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&temp);
        return Err(err.to_string());
    }

    if let Err(err) = rename_with_transient_retry(|| fs::rename(&temp, target)) {
        let _ = fs::remove_file(&temp);
        return Err(err.to_string());
    }

    Ok(())
}

pub(crate) fn is_transient_rename_lock(err: &io::Error) -> bool {
    if err.kind() == io::ErrorKind::PermissionDenied {
        return true;
    }
    // Windows ERROR_ACCESS_DENIED (5) and ERROR_SHARING_VIOLATION (32).
    matches!(err.raw_os_error(), Some(5 | 32))
}

pub(crate) fn rename_with_transient_retry<F>(mut op: F) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
{
    const MAX_RETRIES: u32 = 5;
    let mut attempt = 0;
    loop {
        match op() {
            Ok(()) => return Ok(()),
            Err(err) if attempt < MAX_RETRIES && is_transient_rename_lock(&err) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(u64::from(attempt) * 2));
            }
            Err(err) => return Err(err),
        }
    }
}

/// Validates, hashes, and atomically copies an external audio file into
/// `<data_local_dir>/sounds/<blake3>.<ext>` (`AGENTS.md` §7–8).
pub fn import_audio_copy(source: &Path) -> Result<ImportedSound, String> {
    let base = data_dir().ok_or_else(|| "Could not resolve local data directory".to_string())?;
    import_audio_into_dir(&base.join("sounds"), source)
}

pub fn import_audio_into_dir(sounds_dir: &Path, source: &Path) -> Result<ImportedSound, String> {
    let probe = validate_audio_file(source)?;
    let bytes = fs::read(source).map_err(|e| format!("Failed to read audio file: {e}"))?;
    let hash = hash_bytes(&bytes);
    let stored_file = copy_sound_atomic(sounds_dir, source, &bytes, &hash)?;

    let display_file = source
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_string();

    Ok(ImportedSound {
        stored_file,
        display_file,
        duration_ms: probe.duration_ms,
        audio_format: probe.format_name,
    })
}

/// Deletes a stored sound file from `sounds/` unless another slot still shares
/// the same content hash (`AGENTS.md` §7).
pub fn remove_sound_if_unreferenced(slots: &[SlotConfig], removed_stored_file: &str) {
    if let Some(base) = data_dir() {
        remove_sound_if_unreferenced_in_dir(&base.join("sounds"), slots, removed_stored_file);
    }
}

pub fn remove_sound_if_unreferenced_in_dir(
    sounds_dir: &Path,
    slots: &[SlotConfig],
    removed_stored_file: &str,
) {
    let still_used = slots
        .iter()
        .any(|slot| slot.stored_file.as_deref() == Some(removed_stored_file));
    if !still_used {
        let _ = fs::remove_file(sounds_dir.join(removed_stored_file));
    }
}

/// Resolves a stored sound's relative filename to its path on disk.
pub fn resolve_stored_sound(stored_file: &str) -> Option<PathBuf> {
    data_dir().map(|base| base.join("sounds").join(stored_file))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LibraryDiskStats {
    pub sound_files: usize,
    pub total_bytes: u64,
}

pub fn compute_library_stats() -> LibraryDiskStats {
    let Some(base) = data_dir() else {
        return LibraryDiskStats::default();
    };
    compute_library_stats_in_dir(&base)
}

pub fn compute_library_stats_in_dir(base: &Path) -> LibraryDiskStats {
    let mut stats = LibraryDiskStats::default();

    if let Ok(meta) = fs::metadata(base.join("library.json"))
        && meta.is_file()
    {
        stats.total_bytes = stats.total_bytes.saturating_add(meta.len());
    }

    if let Ok(entries) = fs::read_dir(base.join("sounds")) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                stats.sound_files += 1;
                stats.total_bytes = stats.total_bytes.saturating_add(meta.len());
            }
        }
    }

    stats
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    if bytes < 1024 {
        format!("{bytes} B")
    } else if (bytes as f64) < MB {
        format!("{:.1} KB", (bytes as f64) / KB)
    } else if (bytes as f64) < GB {
        format!("{:.2} MB", (bytes as f64) / MB)
    } else {
        format!("{:.2} GB", (bytes as f64) / GB)
    }
}

pub fn delete_all_data() -> Result<(), String> {
    let base = data_dir().ok_or_else(|| "Could not resolve local data directory".to_string())?;
    delete_all_data_in_dir(&base)
}

pub fn delete_all_data_in_dir(base: &Path) -> Result<(), String> {
    match fs::remove_dir_all(base.join("sounds")) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("Failed to delete sounds folder: {err}")),
    }

    match fs::remove_file(base.join("library.json")) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("Failed to delete library.json: {err}")),
    }

    Ok(())
}

/// Exports the library configuration and all referenced sound files into a
/// portable single-file `.bromapack` archive (`BROMAPK1`).
pub fn export_backup(dest_file: &Path, cfg: &AppConfig) -> Result<usize, String> {
    let base = data_dir().ok_or_else(|| "Could not resolve local data directory".to_string())?;
    export_backup_from_dir(&base, dest_file, cfg)
}

pub fn export_backup_from_dir(
    base: &Path,
    dest_file: &Path,
    cfg: &AppConfig,
) -> Result<usize, String> {
    let sounds_dir = base.join("sounds");
    let mut seen = HashSet::new();
    let mut packed_files: Vec<(String, Vec<u8>)> = Vec::new();

    for slot in &cfg.slots {
        let Some(name) = slot.stored_file.as_deref() else {
            continue;
        };
        if !is_safe_stored_filename(name) || !seen.insert(name.to_string()) {
            continue;
        }
        let sound_path = sounds_dir.join(name);
        match fs::read(&sound_path) {
            Ok(bytes) => packed_files.push((name.to_string(), bytes)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(format!(
                    "Failed to read sound file '{name}' for backup: {err}"
                ));
            }
        }
    }

    let json_bytes = serde_json::to_vec_pretty(cfg)
        .map_err(|e| format!("Failed to serialize library config: {e}"))?;
    let json_len = u32::try_from(json_bytes.len())
        .map_err(|_| "Library config exceeds backup format size limit".to_string())?;
    let file_count = u32::try_from(packed_files.len())
        .map_err(|_| "Sound count exceeds backup format limit".to_string())?;

    let payload_extra: usize = packed_files
        .iter()
        .map(|(n, b)| 2 + n.len() + 8 + b.len())
        .sum();
    let mut archive = Vec::with_capacity(8 + 4 + json_bytes.len() + 4 + payload_extra);
    archive.extend_from_slice(BACKUP_MAGIC);
    archive.extend_from_slice(&json_len.to_le_bytes());
    archive.extend_from_slice(&json_bytes);
    archive.extend_from_slice(&file_count.to_le_bytes());

    for (name, audio_bytes) in &packed_files {
        let name_len = u16::try_from(name.len())
            .map_err(|_| format!("Filename '{name}' exceeds backup name limit"))?;
        let audio_len = audio_bytes.len() as u64;
        archive.extend_from_slice(&name_len.to_le_bytes());
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(&audio_len.to_le_bytes());
        archive.extend_from_slice(audio_bytes);
    }

    if let Some(parent) = dest_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create destination folder: {e}"))?;
    }
    atomic_write_file(dest_file, &archive)?;
    Ok(packed_files.len())
}

/// Restores a `.bromapack` backup archive into the local Broma data folder,
/// verifying `blake3` hashes and probing newly restored audio files.
pub fn import_backup(src_file: &Path) -> Result<AppConfig, String> {
    let base = data_dir().ok_or_else(|| "Could not resolve local data directory".to_string())?;
    import_backup_into_dir(&base, src_file)
}

pub fn import_backup_into_dir(base: &Path, src_file: &Path) -> Result<AppConfig, String> {
    let bytes = fs::read(src_file).map_err(|e| format!("Failed to read backup file: {e}"))?;
    let mut cursor = 0usize;

    let magic = take_slice(&bytes, &mut cursor, BACKUP_MAGIC.len())?;
    if magic != BACKUP_MAGIC {
        return Err("Invalid backup file (unsupported or corrupted header)".to_string());
    }

    let json_len = read_u32_le(&bytes, &mut cursor)? as usize;
    let json_bytes = take_slice(&bytes, &mut cursor, json_len)?;
    let mut cfg: AppConfig = serde_json::from_slice(json_bytes)
        .map_err(|e| format!("Corrupted library metadata in backup: {e}"))?;
    cfg.sanitize();

    let file_count = read_u32_le(&bytes, &mut cursor)?;
    let sounds_dir = base.join("sounds");
    fs::create_dir_all(&sounds_dir)
        .map_err(|e| format!("Failed to prepare sounds directory: {e}"))?;

    for _ in 0..file_count {
        let name_len = usize::from(read_u16_le(&bytes, &mut cursor)?);
        let name_bytes = take_slice(&bytes, &mut cursor, name_len)?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| "Backup contains non-UTF-8 sound filename".to_string())?;
        if !is_safe_stored_filename(name) {
            return Err(format!("Backup contains unsafe sound filename '{name}'"));
        }

        let audio_len_u64 = read_u64_le(&bytes, &mut cursor)?;
        let audio_len = usize::try_from(audio_len_u64)
            .map_err(|_| "Backup sound entry exceeds platform memory limits".to_string())?;
        let audio_bytes = take_slice(&bytes, &mut cursor, audio_len)?;

        let Some((expected_hash, ext)) = name.split_once('.') else {
            return Err(format!("Backup sound '{name}' is missing content hash"));
        };
        if ext.is_empty() || hash_bytes(audio_bytes) != expected_hash {
            return Err(format!(
                "Backup integrity check failed for '{name}' (file is corrupted or tampered)"
            ));
        }

        let dest = sounds_dir.join(name);
        atomic_write_file(&dest, audio_bytes)?;
        if let Err(err) = validate_audio_file(&dest) {
            let _ = fs::remove_file(&dest);
            return Err(format!("Restored sound '{name}' failed validation: {err}"));
        }
    }

    if cursor != bytes.len() {
        return Err("Backup archive contains unexpected trailing bytes".to_string());
    }

    save_config_in_dir(base, &cfg)?;
    Ok(cfg)
}

fn is_safe_stored_filename(name: &str) -> bool {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn take_slice<'a>(buf: &'a [u8], cursor: &mut usize, len: usize) -> Result<&'a [u8], String> {
    let end = cursor
        .checked_add(len)
        .ok_or_else(|| "Backup archive is truncated or malformed".to_string())?;
    let slice = buf
        .get(*cursor..end)
        .ok_or_else(|| "Backup archive is truncated or malformed".to_string())?;
    *cursor = end;
    Ok(slice)
}

fn read_u16_le(buf: &[u8], cursor: &mut usize) -> Result<u16, String> {
    let bytes = take_slice(buf, cursor, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32_le(buf: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let bytes = take_slice(buf, cursor, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64_le(buf: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let bytes = take_slice(buf, cursor, 8)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}
