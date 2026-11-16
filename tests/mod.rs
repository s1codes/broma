//! Dedicated test suite mirroring `src/` 1-to-1.
//!
//! Mounted from `src/main.rs` via `#[cfg(test)]` with `autotests = false` in
//! `Cargo.toml` so `src/` stays 100% free of test blocks while all tests run
//! inside a single fast binary.

pub mod audio;
pub mod common;
pub mod hotkeys;
pub mod storage;
pub mod tray;
