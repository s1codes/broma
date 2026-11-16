//! Tests for `src/storage.rs`.

use std::{fs, io};

use crate::storage::{
    AppConfig, DEFAULT_STREAM_VOLUME, MAX_GRID_ROWS, MAX_SLOTS, MIN_PITCH_PERCENT, SlotConfig,
    compute_library_stats_in_dir, delete_all_data_in_dir, export_backup_from_dir, format_bytes,
    import_audio_into_dir, import_backup_into_dir, remove_sound_if_unreferenced_in_dir,
    rename_with_transient_retry, save_config_in_dir,
};
use crate::tests::common::build_test_wav;

#[test]
fn default_grid_is_three_by_three() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.grid_rows, 3);
    assert_eq!(cfg.slots.len(), 9);
    assert!(cfg.slots.iter().all(SlotConfig::is_empty));
}

#[test]
fn sanitize_clamps_out_of_bounds_grid_and_pitch() {
    let mut cfg = AppConfig {
        grid_rows: 999,
        global_volume: 250,
        microphone: None,
        output_device: None,
        monitor_device: None,
        monitor_volume: 150,
        cable_volume: 180,
        close_to_tray: true,
        slots: vec![SlotConfig {
            custom_volume: Some(180),
            pitch_percent: 10,
            ..SlotConfig::default()
        }],
    };

    cfg.sanitize();

    assert_eq!(cfg.grid_rows, MAX_GRID_ROWS);
    assert_eq!(cfg.slots.len(), MAX_SLOTS);
    assert_eq!(cfg.global_volume, 100);
    assert_eq!(cfg.monitor_volume, 100);
    assert_eq!(cfg.cable_volume, 100);
    assert_eq!(cfg.slots[0].custom_volume, Some(100));
    assert_eq!(cfg.slots[0].pitch_percent, MIN_PITCH_PERCENT);
}

#[test]
fn slot_follows_global_volume_unless_overridden() {
    let mut slot = SlotConfig::default();
    assert_eq!(slot.effective_volume(65), 65);

    slot.custom_volume = Some(40);
    assert_eq!(slot.effective_volume(65), 40);
}

#[test]
fn import_validates_dedupes_and_cleans_up_orphans() {
    let root = std::env::temp_dir().join("broma_test_import_flow");
    let _ = fs::remove_dir_all(&root);
    let src_dir = root.join("src_files");
    let sounds_dir = root.join("sounds");
    fs::create_dir_all(&src_dir).unwrap();

    let wav_bytes = build_test_wav(48_000, 1, 24_000);
    let file_a = src_dir.join("airhorn.wav");
    let file_b = src_dir.join("airhorn_copy.wav");
    fs::write(&file_a, &wav_bytes).unwrap();
    fs::write(&file_b, &wav_bytes).unwrap();

    let imported_a = import_audio_into_dir(&sounds_dir, &file_a).unwrap();
    let imported_b = import_audio_into_dir(&sounds_dir, &file_b).unwrap();

    // Identical content hashes must dedupe to the exact same stored filename.
    assert_eq!(imported_a.stored_file, imported_b.stored_file);
    assert_eq!(imported_a.display_file, "airhorn.wav");
    assert_eq!(imported_b.display_file, "airhorn_copy.wav");
    assert_eq!(imported_a.duration_ms, Some(500));
    assert!(sounds_dir.join(&imported_a.stored_file).exists());

    // When one of two slots sharing the hash is cleared, keep the file on disk.
    let mut slots = vec![
        SlotConfig {
            stored_file: Some(imported_a.stored_file.clone()),
            ..SlotConfig::default()
        },
        SlotConfig {
            stored_file: Some(imported_b.stored_file.clone()),
            ..SlotConfig::default()
        },
    ];
    slots[0] = SlotConfig::default();
    remove_sound_if_unreferenced_in_dir(&sounds_dir, &slots, &imported_a.stored_file);
    assert!(sounds_dir.join(&imported_a.stored_file).exists());

    // Once the last slot referencing the hash is cleared, delete the file.
    slots[1] = SlotConfig::default();
    remove_sound_if_unreferenced_in_dir(&sounds_dir, &slots, &imported_b.stored_file);
    assert!(!sounds_dir.join(&imported_b.stored_file).exists());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn config_round_trips_atomically() {
    let root = std::env::temp_dir().join("broma_test_cfg_roundtrip");
    let _ = fs::remove_dir_all(&root);

    let mut cfg = AppConfig::default();
    cfg.slots[0].name = "GG".to_string();
    cfg.slots[0].duration_ms = Some(1250);
    cfg.slots[0].audio_format = Some("WAV".to_string());

    save_config_in_dir(&root, &cfg).unwrap();
    let raw = fs::read(root.join("library.json")).unwrap();
    let loaded: AppConfig = serde_json::from_slice(&raw).unwrap();
    assert_eq!(loaded.slots[0].name, "GG");
    assert_eq!(loaded.slots[0].duration_ms, Some(1250));
    assert_eq!(loaded.slots[0].audio_format.as_deref(), Some("WAV"));
    assert!(loaded.close_to_tray);

    // Legacy `library.json` files without `close_to_tray` or stream volumes must default cleanly.
    let legacy = r#"{"grid_rows":3,"global_volume":80,"slots":[]}"#;
    let parsed_legacy: AppConfig = serde_json::from_str(legacy).unwrap();
    assert!(parsed_legacy.close_to_tray);
    assert_eq!(parsed_legacy.monitor_device, None);
    assert_eq!(parsed_legacy.monitor_volume, DEFAULT_STREAM_VOLUME);
    assert_eq!(parsed_legacy.cable_volume, DEFAULT_STREAM_VOLUME);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn retries_transient_windows_access_denied_locks_and_leaves_no_tmp_files() {
    // Simulate 3 transient Windows Defender / Search Indexer `PermissionDenied` (os error 5)
    // locks followed by success on the 4th attempt.
    let mut attempts = 0u32;
    rename_with_transient_retry(|| {
        attempts += 1;
        if attempts <= 3 {
            Err(io::Error::from_raw_os_error(5))
        } else {
            Ok(())
        }
    })
    .expect("transient Windows ERROR_ACCESS_DENIED should recover via retry");
    assert_eq!(attempts, 4);

    // Non-transient errors (e.g. NotFound) must fail immediately on attempt 1 without stalling.
    let mut bad_attempts = 0u32;
    let err = rename_with_transient_retry(|| {
        bad_attempts += 1;
        Err(io::Error::new(io::ErrorKind::NotFound, "missing"))
    })
    .expect_err("NotFound must not spin in retry loop");
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    assert_eq!(bad_attempts, 1);

    // Repeated atomic config overwrites must succeed cleanly on both POSIX and Windows NTFS
    // and leave zero `.tmp` files behind.
    let root = std::env::temp_dir().join("broma_test_atomic_overwrite_clean");
    let _ = fs::remove_dir_all(&root);
    let mut cfg = AppConfig::default();
    for i in 0..5 {
        cfg.global_volume = i * 20;
        save_config_in_dir(&root, &cfg).unwrap();
    }

    let entries: Vec<String> = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["library.json"]);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn backup_export_delete_and_import_round_trips_library_and_stats() {
    let root = std::env::temp_dir().join("broma_test_backup_roundtrip");
    let _ = fs::remove_dir_all(&root);
    let data_dir = root.join("data");
    let src_dir = root.join("src_audio");
    let backup_file = root.join("exports").join("broma-backup.bromapack");
    fs::create_dir_all(&src_dir).unwrap();

    let wav_bytes = build_test_wav(48_000, 1, 24_000);
    let src_wav = src_dir.join("horn.wav");
    fs::write(&src_wav, &wav_bytes).unwrap();

    let imported = import_audio_into_dir(&data_dir.join("sounds"), &src_wav).unwrap();

    let mut cfg = AppConfig {
        monitor_device: Some("Headphones".to_string()),
        monitor_volume: 75,
        cable_volume: 60,
        ..AppConfig::default()
    };
    cfg.slots[0] = SlotConfig {
        name: "Horn".to_string(),
        stored_file: Some(imported.stored_file.clone()),
        display_file: Some(imported.display_file.clone()),
        duration_ms: imported.duration_ms,
        audio_format: Some(imported.audio_format.clone()),
        custom_volume: Some(90),
        pitch_percent: 110,
        keybind: Some("F2".to_string()),
    };
    // Duplicate slot sharing the exact same stored_file must pack only 1 unique audio file.
    cfg.slots[1] = SlotConfig {
        name: "Horn 2".to_string(),
        stored_file: Some(imported.stored_file.clone()),
        display_file: Some("horn2.wav".to_string()),
        ..SlotConfig::default()
    };
    save_config_in_dir(&data_dir, &cfg).unwrap();

    let stats_before = compute_library_stats_in_dir(&data_dir);
    assert_eq!(stats_before.sound_files, 1);
    assert!(stats_before.total_bytes > wav_bytes.len() as u64);
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(2048), "2.0 KB");
    assert_eq!(format_bytes(5 * 1024 * 1024), "5.00 MB");
    assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");

    let packed_count = export_backup_from_dir(&data_dir, &backup_file, &cfg).unwrap();
    assert_eq!(packed_count, 1);
    assert!(backup_file.exists());

    delete_all_data_in_dir(&data_dir).unwrap();
    assert!(!data_dir.join("library.json").exists());
    assert!(!data_dir.join("sounds").exists());
    let stats_empty = compute_library_stats_in_dir(&data_dir);
    assert_eq!(stats_empty.sound_files, 0);
    assert_eq!(stats_empty.total_bytes, 0);

    let restored = import_backup_into_dir(&data_dir, &backup_file).unwrap();
    assert_eq!(restored.monitor_device.as_deref(), Some("Headphones"));
    assert_eq!(restored.monitor_volume, 75);
    assert_eq!(restored.cable_volume, 60);
    assert_eq!(restored.slots[0].name, "Horn");
    assert_eq!(
        restored.slots[0].stored_file.as_deref(),
        Some(imported.stored_file.as_str())
    );
    assert_eq!(restored.slots[0].keybind.as_deref(), Some("F2"));

    let restored_sound_path = data_dir.join("sounds").join(&imported.stored_file);
    assert_eq!(fs::read(&restored_sound_path).unwrap(), wav_bytes);
    let stats_after = compute_library_stats_in_dir(&data_dir);
    assert_eq!(stats_after.sound_files, 1);
    assert!(stats_after.total_bytes > wav_bytes.len() as u64);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn backup_import_rejects_tampered_and_corrupted_archives() {
    let root = std::env::temp_dir().join("broma_test_backup_tampered");
    let _ = fs::remove_dir_all(&root);
    let data_dir = root.join("data");
    let restore_dir = root.join("restore");
    let src_dir = root.join("src_audio");
    let backup_file = root.join("valid.bromapack");
    fs::create_dir_all(&src_dir).unwrap();

    let wav_bytes = build_test_wav(48_000, 1, 12_000);
    let src_wav = src_dir.join("blip.wav");
    fs::write(&src_wav, &wav_bytes).unwrap();
    let imported = import_audio_into_dir(&data_dir.join("sounds"), &src_wav).unwrap();

    let mut cfg = AppConfig::default();
    cfg.slots[0].name = "Blip".to_string();
    cfg.slots[0].stored_file = Some(imported.stored_file.clone());
    export_backup_from_dir(&data_dir, &backup_file, &cfg).unwrap();

    // 1. Corrupt magic header
    let mut bad_magic = fs::read(&backup_file).unwrap();
    bad_magic[0] = b'X';
    let bad_magic_path = root.join("bad_magic.bromapack");
    fs::write(&bad_magic_path, &bad_magic).unwrap();
    assert!(import_backup_into_dir(&restore_dir, &bad_magic_path).is_err());

    // 2. Tamper with embedded audio byte -> blake3 hash mismatch must fail cleanly
    let mut tampered = fs::read(&backup_file).unwrap();
    let last_idx = tampered.len() - 1;
    tampered[last_idx] ^= 0xFF;
    let tampered_path = root.join("tampered.bromapack");
    fs::write(&tampered_path, &tampered).unwrap();
    assert!(import_backup_into_dir(&restore_dir, &tampered_path).is_err());
    assert!(
        !restore_dir
            .join("sounds")
            .join(&imported.stored_file)
            .exists(),
        "rejected backup must not leave partial audio files behind"
    );

    // 3. Truncated archive
    let truncated_path = root.join("truncated.bromapack");
    fs::write(&truncated_path, &bad_magic[..10]).unwrap();
    assert!(import_backup_into_dir(&restore_dir, &truncated_path).is_err());

    let _ = fs::remove_dir_all(&root);
}
